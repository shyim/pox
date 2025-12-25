//! Update command - update project dependencies.

use anyhow::{Context, Result};
use clap::Args;
use console::style;
use indicatif::{ProgressBar, ProgressStyle};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use phpx_pm::{
    autoload::{AutoloadConfig, AutoloadGenerator, PackageAutoload, RootPackageInfo},
    http::HttpClient,
    installer::{InstallConfig, InstallationManager},
    json::{ComposerJson, ComposerLock, LockedPackage, LockSource, LockDist, LockAutoload, LockAuthor, LockFunding},
    plugin::PluginRegistry,
    repository::{ComposerRepository, RepositoryManager, get_head_commit},
    solver::{Pool, Policy, Request, Solver},
    Package,
    package::{Autoload, AutoloadPath, Stability},
};

use crate::pm::platform::PlatformInfo;

#[derive(Args, Debug)]
pub struct UpdateArgs {
    /// Packages to update (all if not specified)
    #[arg(value_name = "PACKAGES")]
    pub packages: Vec<String>,

    /// Prefer source installation
    #[arg(long)]
    pub prefer_source: bool,

    /// Prefer dist installation
    #[arg(long)]
    pub prefer_dist: bool,

    /// Run in dry-run mode
    #[arg(long)]
    pub dry_run: bool,

    /// Skip dev dependencies
    #[arg(long)]
    pub no_dev: bool,

    /// Skip autoloader generation
    #[arg(long)]
    pub no_autoloader: bool,

    /// Skip script execution
    #[arg(long)]
    pub no_scripts: bool,

    /// Disable progress output
    #[arg(long)]
    pub no_progress: bool,

    /// Update also dependencies of the listed packages
    #[arg(short = 'w', long)]
    pub with_dependencies: bool,

    /// Update all dependencies including root requirements
    #[arg(short = 'W', long)]
    pub with_all_dependencies: bool,

    /// Prefer stable versions
    #[arg(long)]
    pub prefer_stable: bool,

    /// Prefer lowest versions (for testing)
    #[arg(long)]
    pub prefer_lowest: bool,

    /// Only update the lock file
    #[arg(long)]
    pub lock: bool,

    /// Optimize autoloader
    #[arg(short = 'o', long)]
    pub optimize_autoloader: bool,

    /// Working directory
    #[arg(short = 'd', long, default_value = ".")]
    pub working_dir: PathBuf,

    // Common Composer flags (for compatibility)
    /// Force ANSI output
    #[arg(long)]
    pub ansi: bool,

    /// Disable ANSI output
    #[arg(long)]
    pub no_ansi: bool,

    /// Do not ask any interactive question
    #[arg(short = 'n', long)]
    pub no_interaction: bool,

    /// Do not output any message
    #[arg(short = 'q', long)]
    pub quiet: bool,

    /// Increase verbosity (-v, -vv, -vvv)
    #[arg(short = 'v', long, action = clap::ArgAction::Count)]
    pub verbose: u8,
}

pub async fn execute(args: UpdateArgs) -> Result<i32> {
    let working_dir = args.working_dir.canonicalize()
        .context("Failed to resolve working directory")?;

    // Check for composer.json
    let json_path = working_dir.join("composer.json");
    if !json_path.exists() {
        eprintln!("{} No composer.json found in {}",
            style("Error:").red().bold(),
            working_dir.display()
        );
        return Ok(1);
    }

    // Parse composer.json
    let json_content = std::fs::read_to_string(&json_path)
        .context("Failed to read composer.json")?;
    let composer_json: ComposerJson = serde_json::from_str(&json_content)
        .context("Failed to parse composer.json")?;

    println!("{} Updating dependencies", style("Composer").green().bold());

    if args.dry_run {
        println!("{} Running in dry-run mode", style("Info:").cyan());
    }

    // Create progress spinner
    let spinner = if args.no_progress {
        ProgressBar::hidden()
    } else {
        let pb = ProgressBar::new_spinner();
        pb.set_style(
            ProgressStyle::default_spinner()
                .template("{spinner:.green} {msg}")
                .unwrap(),
        );
        pb.enable_steady_tick(Duration::from_millis(100));
        pb
    };

    spinner.set_message("Loading repositories...");

    // Setup HTTP client
    let _http_client = Arc::new(HttpClient::new()
        .context("Failed to create HTTP client")?);

    // Create repository manager
    let mut repo_manager = RepositoryManager::new();

    // Add packagist as default repository with caching
    let packagist = if let Some(cache_dir) = dirs::cache_dir() {
        ComposerRepository::packagist_with_cache(cache_dir)
    } else {
        ComposerRepository::packagist()
    };
    repo_manager.add_repository(Arc::new(packagist));

    spinner.set_message("Resolving dependencies...");

    // Detect platform (PHP version and extensions)
    spinner.set_message("Detecting platform...");
    let platform = PlatformInfo::detect();

    // Get minimum stability from composer.json (default: stable)
    let minimum_stability: Stability = composer_json.minimum_stability
        .parse()
        .unwrap_or(Stability::Stable);

    // Build package pool with transitive dependencies
    let mut pool = Pool::with_minimum_stability(minimum_stability);

    // Add stability flags from composer.json for per-package overrides
    // In composer.json, these are specified as: "vendor/package": "dev" in the require section
    // with @dev, @alpha, @beta, @RC suffixes in constraints
    // For now, we extract explicit stability flags from constraints like "package": "^1.0@dev"
    for (name, constraint) in &composer_json.require {
        if let Some(stability) = extract_stability_flag(constraint) {
            pool.add_stability_flag(name, stability);
        }
    }
    for (name, constraint) in &composer_json.require_dev {
        if let Some(stability) = extract_stability_flag(constraint) {
            pool.add_stability_flag(name, stability);
        }
    }

    // Add platform packages first (php, ext-*) - these bypass stability filtering
    for pkg in platform.to_packages() {
        pool.add_package(pkg);
    }

    let mut loaded_packages: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut pending_packages: Vec<String> = Vec::new();

    // Start with root requirements
    for (name, _constraint) in &composer_json.require {
        if name != "php" && !name.starts_with("ext-") && !name.starts_with("lib-") {
            pending_packages.push(name.clone());
        }
    }

    if !args.no_dev {
        for (name, _constraint) in &composer_json.require_dev {
            if name != "php" && !name.starts_with("ext-") && !name.starts_with("lib-") {
                pending_packages.push(name.clone());
            }
        }
    }

    // Recursively load packages and their dependencies
    let repo_manager = Arc::new(repo_manager);
    let mut tasks = tokio::task::JoinSet::new();
    // Use a fixed max concurrency to avoid overloading
    const MAX_CONCURRENT_REQUESTS: usize = 50;

    // Process loop: spawn tasks from pending queue, collect results
    loop {
        // 1. Spawn tasks while we have pending packages and capacity
        while tasks.len() < MAX_CONCURRENT_REQUESTS {
            if let Some(name) = pending_packages.pop() {
                let name_lower = name.to_lowercase();
                
                // Skip if already processing or processed
                if loaded_packages.contains(&name_lower) {
                    continue;
                }
                loaded_packages.insert(name_lower);

                let rm = repo_manager.clone();
                let name_clone = name.clone();
                
                spinner.set_message(format!("Loading {}...", name));
                
                tasks.spawn(async move {
                    (name_clone.clone(), rm.find_packages(&name_clone).await)
                });
            } else {
                // No more pending packages to spawn right now
                break;
            }
        }

        // 2. If no tasks running and nothing pending, we are done
        if tasks.is_empty() {
             break;
        }

        // 3. Wait for the next task to complete
        if let Some(res) = tasks.join_next().await {
            match res {
                Ok((_name, packages)) => {
                    // Collect new dependencies from all versions
                    for pkg in &packages {
                        for (dep_name, _) in &pkg.require {
                            if dep_name != "php" && !dep_name.starts_with("ext-") && !dep_name.starts_with("lib-") {
                                let dep_lower = dep_name.to_lowercase();
                                // Only add if we haven't seen it yet
                                // Note: We might add duplicates to pending_packages here if multiple threads find the same dep,
                                // but the check at spawn time (pop) will filter them out.
                                if !loaded_packages.contains(&dep_lower) {
                                    pending_packages.push(dep_name.clone());
                                }
                            }
                        }
                        
                        // Add packages to pool
                        // Use add_package_arc to avoid deep cloning
                        pool.add_package_arc(pkg.clone(), None);
                    }
                }
                Err(e) => {
                    // Task panic or cancelled
                    eprintln!("Warning: Task failed: {}", e);
                }
            }
        }
    }


    // Create solver request
    let mut request = Request::new();

    for (name, constraint) in &composer_json.require {
        if name != "php" && !name.starts_with("ext-") && !name.starts_with("lib-") {
            request.require(name, constraint);
        }
    }

    if !args.no_dev {
        for (name, constraint) in &composer_json.require_dev {
            if name != "php" && !name.starts_with("ext-") && !name.starts_with("lib-") {
                request.require(name, constraint);
            }
        }
    }

    // Solve dependencies
    let policy = Policy::new()
        .prefer_lowest(args.prefer_lowest);
    let solver = Solver::new(&pool, &policy);

    let transaction = match solver.solve(&request) {
        Ok(tx) => tx,
        Err(problems) => {
            spinner.finish_and_clear();
            eprintln!("{} Could not resolve dependencies", style("Error:").red().bold());
            for problem in problems.problems() {
                eprintln!("  {}", problem.describe(&pool));
            }
            return Ok(1);
        }
    };

    spinner.set_message("Installing packages...");

    // Collect packages from transaction (exclude platform packages)
    let packages: Vec<Package> = transaction.installs()
        .map(|p| p.as_ref().clone())
        .filter(|p| p.name != "php" && !p.name.starts_with("ext-"))
        .collect();

    if packages.is_empty() {
        spinner.finish_and_clear();
        println!("{} Nothing to update.", style("Info:").cyan());
        return Ok(0);
    }

    // Generate lock file with proper dev/non-dev separation
    // Build the set of packages reachable from require (non-dev)
    let non_dev_roots: HashSet<String> = composer_json.require.keys()
        .filter(|k| *k != "php" && !k.starts_with("ext-") && !k.starts_with("lib-"))
        .map(|k| k.to_lowercase())
        .collect();

    // Build a dependency graph to find all transitive non-dev dependencies
    let non_dev_packages = find_transitive_dependencies(&packages, &non_dev_roots);

    // Separate packages into dev and non-dev
    let (prod_packages, dev_packages): (Vec<_>, Vec<_>) = packages.iter()
        .partition(|p| non_dev_packages.contains(&p.name.to_lowercase()));

    let lock = ComposerLock {
        content_hash: compute_content_hash(&composer_json),
        packages: prod_packages.iter()
            .map(|p| package_to_locked(p))
            .collect(),
        packages_dev: dev_packages.iter()
            .map(|p| package_to_locked(p))
            .collect(),
        ..Default::default()
    };

    // Write lock file
    if !args.dry_run {
        let lock_content = serde_json::to_string_pretty(&lock)
            .context("Failed to serialize composer.lock")?;
        std::fs::write(working_dir.join("composer.lock"), lock_content)
            .context("Failed to write composer.lock")?;
    }

    if args.lock {
        spinner.finish_and_clear();
        println!("{} Lock file updated", style("Success:").green().bold());
        return Ok(0);
    }

    // Install packages
    let http_client = Arc::new(HttpClient::new()
        .context("Failed to create HTTP client")?);

    let install_config = InstallConfig {
        vendor_dir: working_dir.join("vendor"),
        bin_dir: working_dir.join("vendor/bin"),
        cache_dir: dirs::cache_dir().unwrap_or_else(|| PathBuf::from(".phpx/cache")),
        prefer_source: args.prefer_source,
        prefer_dist: args.prefer_dist || !args.prefer_source,
        dry_run: args.dry_run,
        no_dev: args.no_dev,
    };

    let manager = InstallationManager::new(http_client.clone(), install_config.clone());
    let result = manager.execute(&transaction).await
        .map_err(|e| anyhow::anyhow!("Failed to execute installation: {}", e))?;

    spinner.finish_and_clear();

    // Report results
    for pkg in &result.installed {
        println!("  {} {} ({})",
            style("-").green(),
            style(&pkg.name).white().bold(),
            style(&pkg.version).yellow()
        );
    }

    for (from, to) in &result.updated {
        println!("  {} {} ({} => {})",
            style("-").cyan(),
            style(&to.name).white().bold(),
            style(&from.version).yellow(),
            style(&to.version).green()
        );
    }

    // Generate autoloader
    if !args.no_autoloader && !args.dry_run {
        println!("{} Generating autoload files", style("Info:").cyan());

        // Build alias map (empty for update since we don't have aliases in newly resolved packages)
        let aliases_map: HashMap<String, Vec<String>> = HashMap::new();
        let dev_mode = !args.no_dev;

        // Convert packages to PackageAutoload (all are non-dev after update since we separate later)
        let mut package_autoloads: Vec<PackageAutoload> = lock.packages.iter()
            .map(|lp| locked_package_to_autoload(lp, false, &aliases_map))
            .collect();
        if dev_mode {
            package_autoloads.extend(lock.packages_dev.iter().map(|lp| locked_package_to_autoload(lp, true, &aliases_map)));
        }

        let autoload_config = AutoloadConfig {
            vendor_dir: install_config.vendor_dir.clone(),
            base_dir: working_dir.clone(),
            optimize: args.optimize_autoloader,
            suffix: Some(lock.content_hash.clone()),
            ..Default::default()
        };

        let generator = AutoloadGenerator::new(autoload_config);

        // Get root autoload from composer.json
        let root_autoload: Option<Autoload> = {
            let json: serde_json::Value = serde_json::from_str(&json_content)?;
            json.get("autoload")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
        };

        // Build root package info
        let reference = get_head_commit(&working_dir);
        let root_package = RootPackageInfo {
            name: composer_json.name.clone().unwrap_or_else(|| "__root__".to_string()),
            pretty_version: composer_json.version.clone().unwrap_or_else(|| "dev-main".to_string()),
            version: composer_json.version.clone().unwrap_or_else(|| "dev-main".to_string()),
            reference,
            package_type: composer_json.package_type.clone(),
            aliases: Vec::new(),
            dev_mode,
        };

        generator.generate(&package_autoloads, root_autoload.as_ref(), Some(&root_package))
            .context("Failed to generate autoloader")?;

        // Run plugin hooks (post-autoload-dump)
        let plugin_registry = PluginRegistry::new();
        plugin_registry.run_post_autoload_dump(
            &install_config.vendor_dir,
            &working_dir,
            &composer_json,
            &packages,
        ).context("Failed to run plugin hooks")?;
    }

    println!("{} {} packages updated",
        style("Success:").green().bold(),
        result.installed.len() + result.updated.len()
    );

    Ok(0)
}

/// Convert a LockedPackage to a PackageAutoload
fn locked_package_to_autoload(lp: &LockedPackage, is_dev: bool, aliases_map: &HashMap<String, Vec<String>>) -> PackageAutoload {
    let autoload = convert_lock_autoload(&lp.autoload);

    // Extract requires (filter out platform requirements like php, ext-*)
    let requires: Vec<String> = lp.require.keys()
        .filter(|k| *k != "php" && !k.starts_with("ext-") && !k.starts_with("lib-"))
        .cloned()
        .collect();

    // Get the reference from source or dist
    let reference = lp.source.as_ref()
        .map(|s| s.reference.clone())
        .or_else(|| lp.dist.as_ref().and_then(|d| d.reference.clone()));

    // Get aliases for this package
    let aliases = aliases_map.get(&lp.name).cloned().unwrap_or_default();

    PackageAutoload {
        name: lp.name.clone(),
        autoload,
        install_path: lp.name.clone(),
        requires,
        pretty_version: Some(lp.version.clone()),
        version: Some(lp.version.clone()),
        reference,
        package_type: lp.package_type.clone(),
        dev_requirement: is_dev,
        aliases,
        replaces: lp.replace.clone(),
        provides: lp.provide.clone(),
    }
}

/// Convert LockAutoload to Autoload
fn convert_lock_autoload(lock_autoload: &LockAutoload) -> Autoload {
    let mut autoload = Autoload::default();

    // Convert PSR-4
    for (namespace, value) in &lock_autoload.psr4 {
        let paths = json_value_to_paths(value);
        autoload.psr4.insert(namespace.clone(), paths);
    }

    // Convert PSR-0
    for (namespace, value) in &lock_autoload.psr0 {
        let paths = json_value_to_paths(value);
        autoload.psr0.insert(namespace.clone(), paths);
    }

    // Classmap
    autoload.classmap = lock_autoload.classmap.clone();

    // Files
    autoload.files = lock_autoload.files.clone();

    // Exclude from classmap
    autoload.exclude_from_classmap = lock_autoload.exclude_from_classmap.clone();

    autoload
}

/// Convert JSON value to AutoloadPath
fn json_value_to_paths(value: &serde_json::Value) -> AutoloadPath {
    match value {
        serde_json::Value::String(s) => AutoloadPath::Single(s.clone()),
        serde_json::Value::Array(arr) => {
            let paths: Vec<String> = arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect();
            if paths.len() == 1 {
                AutoloadPath::Single(paths[0].clone())
            } else {
                AutoloadPath::Multiple(paths)
            }
        }
        _ => AutoloadPath::Single(String::new()),
    }
}

/// Convert a Package to a LockedPackage
fn package_to_locked(pkg: &Package) -> LockedPackage {
    // Convert autoload from Package to LockAutoload
    let autoload = pkg.autoload.as_ref().map(|a| {
        LockAutoload {
            psr4: a.psr4.iter().map(|(k, v)| {
                (k.clone(), serde_json::to_value(v).unwrap_or(serde_json::Value::Null))
            }).collect(),
            psr0: a.psr0.iter().map(|(k, v)| {
                (k.clone(), serde_json::to_value(v).unwrap_or(serde_json::Value::Null))
            }).collect(),
            classmap: a.classmap.clone(),
            files: a.files.clone(),
            exclude_from_classmap: a.exclude_from_classmap.clone(),
        }
    }).unwrap_or_default();

    let autoload_dev = pkg.autoload_dev.as_ref().map(|a| {
        LockAutoload {
            psr4: a.psr4.iter().map(|(k, v)| {
                (k.clone(), serde_json::to_value(v).unwrap_or(serde_json::Value::Null))
            }).collect(),
            psr0: a.psr0.iter().map(|(k, v)| {
                (k.clone(), serde_json::to_value(v).unwrap_or(serde_json::Value::Null))
            }).collect(),
            classmap: a.classmap.clone(),
            files: a.files.clone(),
            exclude_from_classmap: a.exclude_from_classmap.clone(),
        }
    }).unwrap_or_default();

    // Convert authors
    let authors = pkg.authors.iter().map(|a| {
        LockAuthor {
            name: a.name.clone().unwrap_or_default(),
            email: a.email.clone(),
            homepage: a.homepage.clone(),
            role: a.role.clone(),
        }
    }).collect();

    // Convert funding
    let funding = pkg.funding.iter().filter_map(|f| {
        Some(LockFunding {
            url: f.url.clone()?,
            funding_type: f.funding_type.clone()?,
        })
    }).collect();

    // Convert support to HashMap
    let mut support = std::collections::HashMap::new();
    if let Some(ref s) = pkg.support {
        if let Some(ref v) = s.issues { support.insert("issues".to_string(), v.clone()); }
        if let Some(ref v) = s.source { support.insert("source".to_string(), v.clone()); }
        if let Some(ref v) = s.docs { support.insert("docs".to_string(), v.clone()); }
        if let Some(ref v) = s.forum { support.insert("forum".to_string(), v.clone()); }
        if let Some(ref v) = s.wiki { support.insert("wiki".to_string(), v.clone()); }
        if let Some(ref v) = s.email { support.insert("email".to_string(), v.clone()); }
        if let Some(ref v) = s.irc { support.insert("irc".to_string(), v.clone()); }
        if let Some(ref v) = s.chat { support.insert("chat".to_string(), v.clone()); }
        if let Some(ref v) = s.security { support.insert("security".to_string(), v.clone()); }
        if let Some(ref v) = s.rss { support.insert("rss".to_string(), v.clone()); }
    }

    LockedPackage {
        name: pkg.name.clone(),
        version: pkg.version.clone(),
        source: pkg.source.as_ref().map(|s| LockSource {
            source_type: s.source_type.clone(),
            url: s.url.clone(),
            reference: s.reference.clone(),
        }),
        dist: pkg.dist.as_ref().map(|d| LockDist {
            dist_type: d.dist_type.clone(),
            url: d.url.clone(),
            reference: d.reference.clone(),
            shasum: d.shasum.clone(),
        }),
        require: pkg.require.clone(),
        require_dev: pkg.require_dev.clone(),
        conflict: pkg.conflict.clone(),
        provide: pkg.provide.clone(),
        replace: pkg.replace.clone(),
        suggest: pkg.suggest.clone(),
        bin: pkg.bin.clone(),
        package_type: pkg.package_type.clone(),
        extra: pkg.extra.clone(),
        autoload,
        autoload_dev,
        notification_url: pkg.notification_url.clone(),
        license: pkg.license.clone(),
        authors,
        description: pkg.description.clone(),
        homepage: pkg.homepage.clone(),
        keywords: pkg.keywords.clone(),
        support,
        funding,
        time: pkg.time.map(|t| t.to_rfc3339()),
        ..Default::default()
    }
}

/// Find all packages that are transitively required from the given root packages.
/// This is used to determine which packages are production dependencies vs dev-only.
fn find_transitive_dependencies(packages: &[Package], roots: &HashSet<String>) -> HashSet<String> {
    // Build a map of package name -> package for quick lookup
    let pkg_map: std::collections::HashMap<String, &Package> = packages.iter()
        .map(|p| (p.name.to_lowercase(), p))
        .collect();

    let mut result: HashSet<String> = HashSet::new();
    let mut queue: std::collections::VecDeque<String> = roots.iter().cloned().collect();

    while let Some(name) = queue.pop_front() {
        if result.contains(&name) {
            continue;
        }

        // Check if this package exists in our resolved set
        if let Some(pkg) = pkg_map.get(&name) {
            result.insert(name.clone());

            // Add all dependencies to the queue
            for (dep_name, _) in &pkg.require {
                let dep_lower = dep_name.to_lowercase();
                // Skip platform requirements
                if dep_lower == "php" || dep_lower.starts_with("ext-") || dep_lower.starts_with("lib-") {
                    continue;
                }
                if !result.contains(&dep_lower) {
                    queue.push_back(dep_lower);
                }
            }
        } else {
            // Package is a root requirement but might not be in our package list
            // (could be a platform package like php or ext-*)
            // Just mark it as visited to avoid infinite loops
            result.insert(name);
        }
    }

    result
}

/// Compute content hash for composer.json
fn compute_content_hash(json: &ComposerJson) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    // Hash the relevant fields
    for (k, v) in &json.require {
        k.hash(&mut hasher);
        v.hash(&mut hasher);
    }
    for (k, v) in &json.require_dev {
        k.hash(&mut hasher);
        v.hash(&mut hasher);
    }
    format!("{:x}", hasher.finish())
}

/// Extract stability flag from a version constraint.
/// Examples: "^1.0@dev" -> Some(Stability::Dev), "^1.0" -> None
fn extract_stability_flag(constraint: &str) -> Option<Stability> {
    // Look for @stability suffix in constraint
    if let Some(at_pos) = constraint.rfind('@') {
        let stability_str = &constraint[at_pos + 1..];
        let stability: Stability = stability_str.parse().ok()?;
        // Only return if it's not the default stable
        if stability != Stability::Stable {
            return Some(stability);
        }
    }
    None
}

mod dirs {
    use std::path::PathBuf;

    pub fn cache_dir() -> Option<PathBuf> {
        #[cfg(target_os = "macos")]
        { std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Caches/phpx")) }

        #[cfg(target_os = "linux")]
        {
            std::env::var_os("XDG_CACHE_HOME")
                .map(PathBuf::from)
                .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
                .map(|p| p.join("phpx"))
        }

        #[cfg(target_os = "windows")]
        { std::env::var_os("LOCALAPPDATA").map(PathBuf::from).map(|p| p.join("phpx")) }

        #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
        { std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".phpx/cache")) }
    }
}
