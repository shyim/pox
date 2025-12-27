use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;
use anyhow::{Context, Result};
use console::style;
use indicatif::{ProgressBar, ProgressStyle};
use tokio::task::JoinSet;

use crate::composer::Composer;
use crate::event::{
    PostAutoloadDumpEvent, PostInstallEvent, PostUpdateEvent,
    PreAutoloadDumpEvent, PreInstallEvent, PreUpdateEvent,
};
use crate::json::{ComposerLock, ComposerJson, LockedPackage};
use crate::package::{Package, Stability, Autoload, detect_root_version, RootVersion};
use crate::solver::{Pool, Policy, Request, Solver};
use crate::autoload::{AutoloadConfig, AutoloadGenerator, PackageAutoload, RootPackageInfo, get_head_commit};
use crate::util::is_platform_package;

pub struct Installer {
    composer: Composer,
}

impl Installer {
    pub fn new(composer: Composer) -> Self {
        Self { composer }
    }

    pub async fn update(&self, optimize_autoloader: bool, update_lock_only: bool) -> Result<i32> {
        let composer_json = &self.composer.composer_json;
        let working_dir = &self.composer.working_dir;
        let install_config = self.composer.installation_manager.config();
        let dry_run = install_config.dry_run;
        let no_dev = install_config.no_dev;
        let prefer_lowest = install_config.prefer_lowest;
        let platform_packages = &self.composer.platform_packages;

        log::debug!("Reading {}/composer.json", working_dir.display());

        println!("{} Updating dependencies", style("Composer").green().bold());

        if dry_run {
            println!("{} Running in dry-run mode", style("Info:").cyan());
        }

        // Dispatch pre-update event
        let exit_code = self.composer.dispatch(&PreUpdateEvent::new(!no_dev))?;
        if exit_code != 0 {
            return Ok(exit_code);
        }

        // Create progress spinner
        let spinner = ProgressBar::new_spinner();
        spinner.set_style(
            ProgressStyle::default_spinner()
                .template("{spinner:.green} {msg}")
                .unwrap(),
        );
        spinner.enable_steady_tick(Duration::from_millis(100));
        spinner.set_message("Loading repositories...");

        // Setup repository manager
        let repo_manager = self.composer.repository_manager.clone();

        spinner.set_message("Resolving dependencies...");

        // Get minimum stability (default to "stable" if not specified)
        let minimum_stability: Stability = composer_json.minimum_stability
            .as_deref()
            .unwrap_or("stable")
            .parse()
            .unwrap_or(Stability::Stable);

        log::debug!("Minimum stability: {:?}", minimum_stability);

        // Detect root package version
        let root_version = get_root_version(working_dir, composer_json);

        // Build package pool
        let mut pool = Pool::with_minimum_stability(minimum_stability);

        // Add root package to pool (for replace/provide/conflict handling)
        // Use add_platform_package to bypass stability filtering (root is always installed)
        let root_pkg = create_root_package(composer_json, &root_version);
        if !root_pkg.replace.is_empty() || !root_pkg.provide.is_empty() {
            log::debug!(
                "Root package version: {} (normalized: {})",
                root_pkg.pretty_version.as_deref().unwrap_or("N/A"),
                root_pkg.version
            );
            log::debug!(
                "Root package replaces: {:?}",
                root_pkg.replace
            );
            log::debug!(
                "Root package provides: {:?}",
                root_pkg.provide
            );
            let root_id = pool.add_platform_package(root_pkg);
            log::debug!("Added root package to pool with id {}", root_id);
        }

        // Collect packages that are replaced/provided by root - we don't need to load these
        // from repositories since the root package satisfies them
        let root_replaced: HashSet<String> = composer_json
            .replace
            .keys()
            .chain(composer_json.provide.keys())
            .map(|s| s.to_lowercase())
            .collect();

        if !root_replaced.is_empty() {
            log::debug!(
                "Skipping repository lookup for root-replaced packages: {:?}",
                root_replaced
            );
        }

        // Add stability flags
        for (name, constraint) in &composer_json.require {
            if let Some(stability) = extract_stability_flag(constraint) {
                pool.add_stability_flag(name, stability);
                log::trace!("Stability flag for {}: {:?}", name, stability);
            }
        }
        for (name, constraint) in &composer_json.require_dev {
            if let Some(stability) = extract_stability_flag(constraint) {
                pool.add_stability_flag(name, stability);
                log::trace!("Stability flag for {}: {:?}", name, stability);
            }
        }

        // Add platform packages (bypass stability filtering - these are fixed system packages)
        for pkg in platform_packages {
            log::debug!("Platform package: {} {}", pkg.name, pkg.version);
            pool.add_platform_package(pkg.clone());
        }

        // Load packages with constraint-based filtering
        // This dramatically reduces the pool size by only loading versions that could
        // possibly be selected, similar to PHP Composer's demand-driven loading.
        let load_start = std::time::Instant::now();

        // Track loaded packages and pending packages with their constraints
        // Key = lowercase package name, Value = merged constraint string
        let mut loaded_packages: HashSet<String> = root_replaced.clone();
        let mut pending_packages: HashMap<String, String> = HashMap::new();
        let mut http_request_count = 0usize;

        // Collect all packages first, then sort and add to pool for deterministic order
        let mut all_packages: Vec<Arc<Package>> = Vec::new();

        // Add root requirements with their constraints
        for (name, constraint) in &composer_json.require {
            if !is_platform_package(name) && !root_replaced.contains(&name.to_lowercase()) {
                let name_lower = name.to_lowercase();
                pending_packages.insert(name_lower, constraint.clone());
            }
        }
        if !no_dev {
            for (name, constraint) in &composer_json.require_dev {
                if !is_platform_package(name) && !root_replaced.contains(&name.to_lowercase()) {
                    let name_lower = name.to_lowercase();
                    // Merge constraints if already present
                    if let Some(existing) = pending_packages.get(&name_lower) {
                        pending_packages.insert(name_lower, format!("{} || {}", existing, constraint));
                    } else {
                        pending_packages.insert(name_lower, constraint.clone());
                    }
                }
            }
        }

        let mut tasks = JoinSet::new();
        const MAX_CONCURRENT_REQUESTS: usize = 50;

        loop {
            // Get pending packages sorted for deterministic processing
            let mut pending_list: Vec<(String, String)> = pending_packages.drain().collect();
            pending_list.sort_by(|a, b| a.0.cmp(&b.0));

            while tasks.len() < MAX_CONCURRENT_REQUESTS {
                if let Some((name, constraint)) = pending_list.pop() {
                    if loaded_packages.contains(&name) {
                        continue;
                    }
                    loaded_packages.insert(name.clone());

                    let rm = repo_manager.clone();
                    let name_clone = name.clone();
                    let constraint_clone = constraint.clone();

                    spinner.set_message(format!("Loading {}...", name));
                    http_request_count += 1;

                    tasks.spawn(async move {
                        let start = std::time::Instant::now();
                        // Use constraint-based loading when available
                        let result = rm.find_packages_with_constraint(&name_clone, &constraint_clone).await;
                        (name_clone.clone(), constraint_clone, result, start.elapsed())
                    });
                } else {
                    break;
                }
            }

            // Put remaining items back
            for (name, constraint) in pending_list {
                pending_packages.insert(name, constraint);
            }

            if tasks.is_empty() && pending_packages.is_empty() {
                break;
            }

            if let Some(res) = tasks.join_next().await {
                match res {
                    Ok((name, _constraint, packages, elapsed)) => {
                        log::trace!("HTTP: {} ({} versions) in {:?}", name, packages.len(), elapsed);
                        for pkg in &packages {
                            // Add dependencies with their constraints
                            for (dep_name, dep_constraint) in &pkg.require {
                                if !is_platform_package(dep_name) {
                                    let dep_lower = dep_name.to_lowercase();
                                    if !loaded_packages.contains(&dep_lower) {
                                        // Merge constraint if already pending
                                        if let Some(existing) = pending_packages.get(&dep_lower) {
                                            // Only extend if the new constraint isn't a subset
                                            // For simplicity, just merge with OR
                                            pending_packages.insert(
                                                dep_lower,
                                                format!("{} || {}", existing, dep_constraint),
                                            );
                                        } else {
                                            log::trace!("Adding dependency {} {} from {} {}", dep_name, dep_constraint, pkg.name, pkg.version);
                                            pending_packages.insert(dep_lower, dep_constraint.clone());
                                        }
                                    }
                                }
                            }
                            // Collect packages instead of adding directly to pool
                            all_packages.push(pkg.clone());
                        }
                    }
                    Err(e) => eprintln!("Warning: Task failed: {}", e),
                }
            }
        }

        // Sort packages by name and version for deterministic pool order
        all_packages.sort_by(|a, b| {
            match a.name.cmp(&b.name) {
                std::cmp::Ordering::Equal => a.version.cmp(&b.version),
                other => other,
            }
        });

        // Add sorted packages to pool
        for pkg in all_packages {
            pool.add_package_arc(pkg, None);
        }

        log::info!("Loaded {} packages ({} HTTP requests) in {:?}",
            pool.len(), http_request_count, load_start.elapsed());
        log::debug!("Pool has {} packages after loading", pool.len());

        // Solver Request
        let mut request = Request::new();
        for (name, constraint) in &composer_json.require {
            if !is_platform_package(name) {
                request.require(name, constraint);
            }
        }
        if !no_dev {
            for (name, constraint) in &composer_json.require_dev {
                if !is_platform_package(name) {
                    request.require(name, constraint);
                }
            }
        }

        // Add root package as fixed if it has replace/provide
        // This ensures the solver knows the root package is always installed
        // and its replaced/provided packages are available
        let root_pkg = create_root_package(composer_json, &root_version);
        if !root_pkg.replace.is_empty() || !root_pkg.provide.is_empty() {
            request.fix(root_pkg);
        }

        let policy = Policy::new().prefer_lowest(prefer_lowest);
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

        let packages: Vec<Package> = transaction.installs()
            .map(|p| p.as_ref().clone())
            .filter(|p| !is_platform_package(&p.name))
            .collect();

        if packages.is_empty() {
            spinner.finish_and_clear();
            println!("{} Nothing to update.", style("Info:").cyan());
            return Ok(0);
        }

        // Generate lock file content
        let non_dev_roots: HashSet<String> = composer_json.require.keys()
            .filter(|k| !is_platform_package(k))
            .map(|k| k.to_lowercase())
            .collect();

        let non_dev_packages = find_transitive_dependencies(&packages, &non_dev_roots);
        let (prod_packages, dev_packages): (Vec<_>, Vec<_>) = packages.iter()
            .partition(|p| non_dev_packages.contains(&p.name.to_lowercase()));

        // Count operations for logging
        let install_count = packages.len();
        let update_count = 0; // TODO: track updates vs installs properly
        let removal_count = 0; // TODO: track removals
        log::info!("Lock file operations: {} installs, {} updates, {} removals",
            install_count, update_count, removal_count);

        let lock = ComposerLock {
            content_hash: compute_content_hash(composer_json),
            packages: prod_packages.iter().map(|p| LockedPackage::from(*p)).collect(),
            packages_dev: dev_packages.iter().map(|p| LockedPackage::from(*p)).collect(),
            ..Default::default()
        };

        if !dry_run {
            log::debug!("Writing lock file");
            let lock_content = serde_json::to_string_pretty(&lock).context("Failed to serialize composer.lock")?;
            std::fs::write(working_dir.join("composer.lock"), lock_content).context("Failed to write composer.lock")?;
        }

        if update_lock_only {
             spinner.finish_and_clear();
             println!("{} Lock file updated", style("Success:").green().bold());
             return Ok(0);
        }

        log::debug!("Installing dependencies from lock file");
        log::info!("Package operations: {} installs, {} updates, {} removals",
            install_count, update_count, removal_count);

        // Install
        let manager = &self.composer.installation_manager;
        // Hack: currently manager executes transaction, but we created a simplified transaction/list for lock
        // In original update.rs it calls manager.execute(&transaction).
        let result = manager.execute(&transaction).await
            .map_err(|e| anyhow::anyhow!("Failed to execute installation: {}", e))?;

        spinner.finish_and_clear();

        // Report
        for pkg in &result.installed {
            log::debug!("Installed {} ({})", pkg.name, pkg.version);
            println!("  {} {} ({})", style("-").green(), style(&pkg.name).white().bold(), style(&pkg.version).yellow());
        }
        for (from, to) in &result.updated {
            log::debug!("Updated {} ({} => {})", to.name, from.version, to.version);
            println!("  {} {} ({} => {})", style("-").cyan(), style(&to.name).white().bold(), style(&from.version).yellow(), style(&to.version).green());
        }

        // Autoload
        if !dry_run { // check no_autoloader in args from caller? We don't have that arg here yet.
             // We can assume if they called update() they want autoloader unless we add a flag.
             // For now, I'll assume YES unless I add the flag to method signature.
             // Added optimize_autoloader flag. I should add `no_autoloader` too?
             // Lets assume we do it.
             println!("{} Generating autoload files", style("Info:").cyan());
             
             let aliases_map: HashMap<String, Vec<String>> = HashMap::new(); // Empty for clean update
             let dev_mode = !no_dev;

             let mut package_autoloads: Vec<PackageAutoload> = lock.packages.iter()
                .map(|lp| locked_package_to_autoload(lp, false, &aliases_map))
                .collect();
             if dev_mode {
                 package_autoloads.extend(lock.packages_dev.iter().map(|lp| locked_package_to_autoload(lp, true, &aliases_map)));
             }

             let autoload_config = AutoloadConfig {
                 vendor_dir: manager.config().vendor_dir.clone(),
                 base_dir: working_dir.clone(),
                 optimize: optimize_autoloader,
                 suffix: Some(lock.content_hash.clone()),
                 ..Default::default()
             };

             let generator = AutoloadGenerator::new(autoload_config);

             let root_autoload: Option<Autoload> = Some(composer_json.autoload.clone().into());

             let root_package = create_root_package_info(
                 composer_json,
                 &root_version,
                 working_dir,
                 Vec::new(),
                 dev_mode,
             );

             generator.generate(&package_autoloads, root_autoload.as_ref(), Some(&root_package))
                 .context("Failed to generate autoloader")?;

             // Dispatch post-autoload-dump event (runs scripts and plugins)
             let arc_packages: Vec<Arc<Package>> = packages.iter().map(|p| Arc::new(p.clone())).collect();
             let event = PostAutoloadDumpEvent::new(arc_packages, !no_dev, optimize_autoloader);
             let exit_code = self.composer.dispatch(&event)?;
             if exit_code != 0 {
                 return Ok(exit_code);
             }
        }

        println!("{} {} packages updated", style("Success:").green().bold(), result.installed.len() + result.updated.len());

        // Dispatch post-update event
        if !dry_run {
            let exit_code = self.composer.dispatch(&PostUpdateEvent::new(!no_dev))?;
            if exit_code != 0 {
                return Ok(exit_code);
            }
        }

        Ok(0)
    }

    pub async fn install(&self, no_scripts: bool, optimize_autoloader: bool, _classmap_authoritative: bool, _apcu_autoloader: bool, _ignore_platform_reqs: bool) -> Result<i32> {
        let composer_json = &self.composer.composer_json;
        let working_dir = &self.composer.working_dir;
        let install_config = self.composer.installation_manager.config();
        let dry_run = install_config.dry_run;
        let no_dev = install_config.no_dev;
        let lock = self.composer.composer_lock.as_ref().context("No composer.lock file found")?;

        // Detect root package version
        let root_version = get_root_version(working_dir, composer_json);

        // Dispatch pre-install event
        if !no_scripts {
             let exit_code = self.composer.dispatch(&PreInstallEvent::new(!no_dev))?;
             if exit_code != 0 { return Ok(exit_code); }
        }

        // Convert locked packages
        let mut packages: Vec<Package> = lock.packages.iter().map(Package::from).collect();
        if !no_dev {
            packages.extend(lock.packages_dev.iter().map(Package::from));
        }

        if packages.is_empty() {
             println!("{} Nothing to install.", style("Info:").cyan());
             return Ok(0);
        }

        println!("{} Installing dependencies from lock file", style("Composer").green().bold());
        if dry_run { println!("{} Running in dry-run mode", style("Info:").cyan()); }

        let progress = ProgressBar::new(packages.len() as u64);
        progress.set_style(ProgressStyle::default_bar().template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} {msg}").unwrap().progress_chars("#>-"));
        progress.enable_steady_tick(Duration::from_millis(100));

        let manager = &self.composer.installation_manager;
        let result = manager.install_packages(&packages).await.context("Failed to install packages")?;

        progress.finish_and_clear();

        if !result.installed.is_empty() {
             for pkg in &result.installed {
                 println!("  {} {} ({})", style("-").green(), style(&pkg.name).white().bold(), style(&pkg.version).yellow());
             }
        }

        if !dry_run {
             // Dispatch pre-autoload-dump event
             if !no_scripts {
                 let exit_code = self.composer.dispatch(&PreAutoloadDumpEvent::new(!no_dev, optimize_autoloader))?;
                 if exit_code != 0 { return Ok(exit_code); }
             }

             println!("{} Generating autoload files", style("Info:").cyan());
             
             let mut aliases_map: HashMap<String, Vec<String>> = HashMap::new();
             for alias in &lock.aliases {
                 aliases_map.entry(alias.package.clone()).or_default().push(alias.alias.clone());
             }
             let dev_mode = !no_dev;
             let mut package_autoloads: Vec<PackageAutoload> = lock.packages.iter()
                 .map(|lp| locked_package_to_autoload(lp, false, &aliases_map))
                 .collect();
             if dev_mode {
                 package_autoloads.extend(lock.packages_dev.iter().map(|lp| locked_package_to_autoload(lp, true, &aliases_map)));
             }
             
             let autoload_config = AutoloadConfig {
                 vendor_dir: manager.config().vendor_dir.clone(),
                 base_dir: working_dir.clone(),
                 optimize: optimize_autoloader,
                 suffix: if !lock.content_hash.is_empty() { Some(lock.content_hash.clone()) } else { None },
                 ..Default::default()
             };

             let generator = AutoloadGenerator::new(autoload_config);
             // Root autoload from json
             let root_autoload: Option<Autoload> = Some(composer_json.autoload.clone().into());
             let root_aliases = aliases_map
                 .get(&composer_json.name.clone().unwrap_or_default())
                 .cloned()
                 .unwrap_or_default();
             let root_package = create_root_package_info(
                 composer_json,
                 &root_version,
                 working_dir,
                 root_aliases,
                 dev_mode,
             );

             generator.generate(&package_autoloads, root_autoload.as_ref(), Some(&root_package)).context("Failed to generate autoloader")?;

             // Dispatch post-autoload-dump event (runs scripts and plugins)
             if !no_scripts {
                 let arc_packages: Vec<Arc<Package>> = packages.iter().map(|p| Arc::new(p.clone())).collect();
                 let event = PostAutoloadDumpEvent::new(arc_packages, dev_mode, optimize_autoloader);
                 let exit_code = self.composer.dispatch(&event)?;
                 if exit_code != 0 { return Ok(exit_code); }
             }
        }

        println!("{} {} packages installed", style("Success:").green().bold(), result.installed.len());

        // Dispatch post-install event
        if !no_scripts && !dry_run {
             let exit_code = self.composer.dispatch(&PostInstallEvent::new(!no_dev))?;
             if exit_code != 0 { return Ok(exit_code); }
        }

        Ok(0)
    }

    pub fn dump_autoload(&self, optimize: bool, authoritative: bool, apcu: bool, no_dev: bool) -> Result<()> {
        let composer_json = &self.composer.composer_json;
        let working_dir = &self.composer.working_dir;
        let manager = &self.composer.installation_manager;

        // Detect root package version
        let root_version = get_root_version(working_dir, composer_json);

        println!("{} Generating autoload files", style("Info:").cyan());
            
        let mut aliases_map: HashMap<String, Vec<String>> = HashMap::new();
        let mut package_autoloads: Vec<PackageAutoload> = Vec::new();
        let mut all_installed_packages: Vec<Package> = Vec::new();
        let dev_mode = !no_dev;
        let mut suffix = None;

        if let Some(lock) = &self.composer.composer_lock {
            for alias in &lock.aliases {
                aliases_map.entry(alias.package.clone()).or_default().push(alias.alias.clone());
            }
            
            package_autoloads = lock.packages.iter()
                .map(|lp| locked_package_to_autoload(lp, false, &aliases_map))
                .collect();
            if dev_mode {
                package_autoloads.extend(lock.packages_dev.iter().map(|lp| locked_package_to_autoload(lp, true, &aliases_map)));
            }
            
            all_installed_packages = lock.packages.iter().map(Package::from).collect();
            if dev_mode {
                all_installed_packages.extend(lock.packages_dev.iter().map(Package::from));
            }
            
            if !lock.content_hash.is_empty() {
                suffix = Some(lock.content_hash.clone());
            }
        }

        let autoload_config = AutoloadConfig {
            vendor_dir: manager.config().vendor_dir.clone(),
            base_dir: working_dir.clone(),
            optimize: optimize || authoritative,
            authoritative,
            apcu,
            suffix,
            ..Default::default()
        };

        let generator = AutoloadGenerator::new(autoload_config);
        // Root autoload from json
        let root_autoload: Option<Autoload> = Some(composer_json.autoload.clone().into());
        let root_aliases = aliases_map
            .get(&composer_json.name.clone().unwrap_or_default())
            .cloned()
            .unwrap_or_default();
        let root_package = create_root_package_info(
            composer_json,
            &root_version,
            working_dir,
            root_aliases,
            dev_mode,
        );

        generator.generate(&package_autoloads, root_autoload.as_ref(), Some(&root_package)).context("Failed to generate autoloader")?;

        // Dispatch post-autoload-dump event (runs scripts and plugins)
        let arc_packages: Vec<Arc<Package>> = all_installed_packages.iter().map(|p| Arc::new(p.clone())).collect();
        let event = PostAutoloadDumpEvent::new(arc_packages, dev_mode, optimize || authoritative);
        self.composer.dispatch(&event)?;

        if optimize || authoritative {
            println!("{} Generated optimized autoload files", style("Success:").green().bold());
        } else {
            println!("{} Generated autoload files", style("Success:").green().bold());
        }

        Ok(())
    }
}

// Helpers

/// Detects and returns the root package version with logging.
///
/// This handles:
/// 1. COMPOSER_ROOT_VERSION environment variable
/// 2. Explicit version in composer.json
/// 3. Branch alias matching current git branch
/// 4. Git branch name as dev version
fn get_root_version(working_dir: &std::path::Path, composer_json: &ComposerJson) -> RootVersion {
    let branch_aliases = composer_json.get_branch_aliases();
    let root_version = detect_root_version(
        working_dir,
        composer_json.version.as_deref(),
        &branch_aliases,
    );

    log::info!(
        "Root package version: {} (from {})",
        root_version.pretty_version,
        root_version.source
    );

    root_version
}

/// Creates a root package that can be added to the solver pool.
///
/// This creates a Package with the root's replace/provide/conflict declarations
/// so the solver knows what virtual packages the root provides.
fn create_root_package(composer_json: &ComposerJson, root_version: &RootVersion) -> Package {
    let name = composer_json
        .name
        .clone()
        .unwrap_or_else(|| "__root__".to_string());

    let mut pkg = Package::new(&name, &root_version.version);
    pkg.pretty_version = Some(root_version.pretty_version.clone());
    pkg.package_type = composer_json.package_type.clone();

    // Copy replace/provide/conflict from composer.json
    pkg.replace = composer_json.replace.clone();
    pkg.provide = composer_json.provide.clone();
    pkg.conflict = composer_json.conflict.clone();

    // Replace self.version with the actual root version
    pkg.replace_self_version();

    pkg
}

/// Creates a RootPackageInfo for autoload generation.
fn create_root_package_info(
    composer_json: &ComposerJson,
    root_version: &RootVersion,
    working_dir: &std::path::Path,
    aliases: Vec<String>,
    dev_mode: bool,
) -> RootPackageInfo {
    RootPackageInfo {
        name: composer_json
            .name
            .clone()
            .unwrap_or_else(|| "__root__".to_string()),
        pretty_version: root_version.pretty_version.clone(),
        version: root_version.version.clone(),
        reference: get_head_commit(working_dir),
        package_type: composer_json.package_type.clone(),
        aliases,
        dev_mode,
    }
}

fn extract_stability_flag(constraint: &str) -> Option<Stability> {
    if let Some(at_pos) = constraint.rfind('@') {
        let stability_str = &constraint[at_pos + 1..];
        let stability: Stability = stability_str.parse().ok()?;
        if stability != Stability::Stable {
            return Some(stability);
        }
    }
    None
}

fn find_transitive_dependencies(packages: &[Package], roots: &HashSet<String>) -> HashSet<String> {
    let pkg_map: HashMap<String, &Package> = packages.iter()
        .map(|p| (p.name.to_lowercase(), p))
        .collect();

    let mut result: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<String> = roots.iter().cloned().collect();

    while let Some(name) = queue.pop_front() {
        if result.contains(&name) {
            continue;
        }

        if let Some(pkg) = pkg_map.get(&name) {
            result.insert(name.clone());
            for (dep_name, _) in &pkg.require {
                if !is_platform_package(dep_name) {
                    let dep_lower = dep_name.to_lowercase();
                    if !result.contains(&dep_lower) {
                        queue.push_back(dep_lower);
                    }
                }
            }
        } else {
            result.insert(name);
        }
    }
    result
}

/// Computes the content-hash for the lock file.
///
/// This matches Composer's algorithm:
/// 1. Extract relevant keys from composer.json
/// 2. Sort keys alphabetically
/// 3. JSON encode (compact, no pretty print)
/// 4. MD5 hash
fn compute_content_hash(json: &crate::json::ComposerJson) -> String {
    use md5::Md5;
    use md5::Digest;
    use serde_json::{json, Map, Value};
    use std::collections::BTreeMap;

    // Build a map of relevant content, using BTreeMap for sorted keys
    let mut relevant: BTreeMap<&str, Value> = BTreeMap::new();

    // Add fields in the order Composer checks them (but BTreeMap will sort alphabetically)
    if let Some(ref name) = json.name {
        relevant.insert("name", json!(name));
    }
    if let Some(ref version) = json.version {
        relevant.insert("version", json!(version));
    }
    if !json.require.is_empty() {
        // Sort the require map for consistent output
        let sorted: BTreeMap<_, _> = json.require.iter().collect();
        relevant.insert("require", json!(sorted));
    }
    if !json.require_dev.is_empty() {
        let sorted: BTreeMap<_, _> = json.require_dev.iter().collect();
        relevant.insert("require-dev", json!(sorted));
    }
    if !json.conflict.is_empty() {
        let sorted: BTreeMap<_, _> = json.conflict.iter().collect();
        relevant.insert("conflict", json!(sorted));
    }
    if !json.replace.is_empty() {
        let sorted: BTreeMap<_, _> = json.replace.iter().collect();
        relevant.insert("replace", json!(sorted));
    }
    if !json.provide.is_empty() {
        let sorted: BTreeMap<_, _> = json.provide.iter().collect();
        relevant.insert("provide", json!(sorted));
    }
    if let Some(ref min_stability) = json.minimum_stability {
        relevant.insert("minimum-stability", json!(min_stability));
    }
    if let Some(prefer_stable) = json.prefer_stable {
        relevant.insert("prefer-stable", json!(prefer_stable));
    }
    if !json.repositories.is_none() {
        // Serialize repositories as-is
        relevant.insert("repositories", serde_json::to_value(&json.repositories).unwrap_or(Value::Null));
    }
    if !json.extra.is_null() {
        relevant.insert("extra", json.extra.clone());
    }
    // Add config.platform if it exists
    if let Some(ref platform) = json.config.platform {
        if !platform.is_empty() {
            let mut config_obj = Map::new();
            config_obj.insert("platform".to_string(), serde_json::to_value(platform).unwrap_or(Value::Null));
            relevant.insert("config", Value::Object(config_obj));
        }
    }

    // JSON encode without pretty printing (compact)
    // PHP's json_encode escapes forward slashes by default, so we need to match that
    let json_str = serde_json::to_string(&relevant)
        .unwrap_or_default()
        .replace("/", "\\/");

    // MD5 hash
    let mut hasher = Md5::new();
    hasher.update(json_str.as_bytes());
    let result = hasher.finalize();
    format!("{:x}", result)
}

fn locked_package_to_autoload(lp: &LockedPackage, is_dev: bool, aliases_map: &HashMap<String, Vec<String>>) -> PackageAutoload {
    let autoload = Autoload::from(&lp.autoload);
    let requires: Vec<String> = lp.require.keys().filter(|k| !is_platform_package(k)).cloned().collect();
    let reference = lp.source.as_ref().map(|s| s.reference.clone()).or_else(|| lp.dist.as_ref().and_then(|d| d.reference.clone()));
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

