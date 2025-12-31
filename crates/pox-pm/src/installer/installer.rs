use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;
use anyhow::{Context, Result};
use console::style;
use indicatif::{ProgressBar, ProgressStyle};
use indexmap::IndexMap;

use crate::composer::Composer;
use crate::event::{
    PostAutoloadDumpEvent, PostInstallEvent, PostUpdateEvent,
    PreAutoloadDumpEvent, PreInstallEvent, PreUpdateEvent,
};
use crate::json::{ComposerLock, ComposerJson, LockedPackage};
use crate::package::{Package, Stability, Autoload, detect_root_version, RootVersion};
use crate::solver::{Pool, Policy, Request, Solver, Transaction};
use crate::autoload::{AutoloadConfig, AutoloadGenerator, PackageAutoload, RootPackageInfo, get_head_commit};
use crate::util::is_platform_package;

pub struct Installer {
    composer: Composer,
}

impl Installer {
    pub fn new(composer: Composer) -> Self {
        Self { composer }
    }

    pub async fn update(&self, optimize_autoloader: bool, update_lock_only: bool, update_packages: Option<Vec<String>>) -> Result<i32> {
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

        // Add stability flags - sort for deterministic order
        let mut sorted_require: Vec<_> = composer_json.require.iter().collect();
        sorted_require.sort_by(|a, b| a.0.cmp(b.0));
        for (name, constraint) in sorted_require {
            if let Some(stability) = extract_stability_flag(constraint) {
                pool.add_stability_flag(name, stability);
                log::trace!("Stability flag for {}: {:?}", name, stability);
            }
        }
        let mut sorted_require_dev: Vec<_> = composer_json.require_dev.iter().collect();
        sorted_require_dev.sort_by(|a, b| a.0.cmp(b.0));
        for (name, constraint) in sorted_require_dev {
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

        // Add root requirements with their constraints - sort for deterministic order
        let mut sorted_require: Vec<_> = composer_json.require.iter().collect();
        sorted_require.sort_by(|a, b| a.0.cmp(b.0));
        for (name, constraint) in sorted_require {
            if !is_platform_package(name) && !root_replaced.contains(&name.to_lowercase()) {
                let name_lower = name.to_lowercase();
                pending_packages.insert(name_lower, constraint.clone());
            }
        }
        if !no_dev {
            let mut sorted_require_dev: Vec<_> = composer_json.require_dev.iter().collect();
            sorted_require_dev.sort_by(|a, b| a.0.cmp(b.0));
            for (name, constraint) in sorted_require_dev {
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

        // Process packages in parallel batches for performance
        // Determinism is ensured by:
        // 1. Processing batches in sorted order
        // 2. Sorting packages before adding to pool
        // 3. Sorting HashMap iterations in rule generation
        loop {
            // Get pending packages sorted for deterministic batch processing
            let mut pending_list: Vec<(String, String)> = pending_packages.drain().collect();
            if pending_list.is_empty() {
                break;
            }
            pending_list.sort_by(|a, b| a.0.cmp(&b.0));

            // Filter out already loaded packages
            let to_load: Vec<(String, String)> = pending_list
                .into_iter()
                .filter(|(name, _)| !loaded_packages.contains(name))
                .collect();

            if to_load.is_empty() {
                continue;
            }

            // Mark all as loaded before parallel fetch to avoid duplicates
            for (name, _) in &to_load {
                loaded_packages.insert(name.clone());
            }

            spinner.set_message(format!("Loading {} packages...", to_load.len()));
            http_request_count += to_load.len();

            // Load packages in parallel
            let mut tasks = tokio::task::JoinSet::new();
            for (name, constraint) in to_load {
                let repo_manager = repo_manager.clone();
                tasks.spawn(async move {
                    let packages = repo_manager.find_packages_with_constraint(&name, &constraint).await;
                    (name, packages)
                });
            }

            // Collect results and process dependencies
            let mut batch_packages: Vec<Arc<Package>> = Vec::new();
            let mut new_deps: Vec<(String, String)> = Vec::new();

            while let Some(result) = tasks.join_next().await {
                if let Ok((name, packages)) = result {
                    log::trace!("HTTP: {} ({} versions)", name, packages.len());
                    for pkg in packages {
                        // Collect dependencies
                        for (dep_name, dep_constraint) in &pkg.require {
                            if !is_platform_package(dep_name) {
                                let dep_lower = dep_name.to_lowercase();
                                if !loaded_packages.contains(&dep_lower) {
                                    log::trace!("Adding dependency {} {} from {} {}", dep_name, dep_constraint, pkg.name, pkg.version);
                                    new_deps.push((dep_lower, dep_constraint.clone()));
                                }
                            }
                        }
                        batch_packages.push(pkg);
                    }
                }
            }

            // Merge new dependencies into pending (after parallel fetch completes)
            // Sort first for deterministic merging
            new_deps.sort_by(|a, b| a.0.cmp(&b.0));
            for (dep_name, dep_constraint) in new_deps {
                if !loaded_packages.contains(&dep_name) {
                    if let Some(existing) = pending_packages.get(&dep_name) {
                        pending_packages.insert(
                            dep_name,
                            format!("{} || {}", existing, dep_constraint),
                        );
                    } else {
                        pending_packages.insert(dep_name, dep_constraint);
                    }
                }
            }

            all_packages.extend(batch_packages);
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

        // Solver Request - sort for deterministic order
        let mut request = Request::new();
        let mut sorted_require: Vec<_> = composer_json.require.iter().collect();
        sorted_require.sort_by(|a, b| a.0.cmp(b.0));
        for (name, constraint) in sorted_require {
            if !is_platform_package(name) {
                request.require(name, constraint);
            }
        }
        if !no_dev {
            let mut sorted_require_dev: Vec<_> = composer_json.require_dev.iter().collect();
            sorted_require_dev.sort_by(|a, b| a.0.cmp(b.0));
            for (name, constraint) in sorted_require_dev {
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

        let preferred_versions = match (&update_packages, &self.composer.composer_lock) {
            (Some(packages_to_update), Some(lock)) if !packages_to_update.is_empty() => {
                let update_allowlist: HashSet<String> = packages_to_update
                    .iter()
                    .map(|p| p.to_lowercase())
                    .collect();

                let mut preferred = HashMap::new();
                for pkg in lock.packages.iter().chain(lock.packages_dev.iter()) {
                    let pkg_name_lower = pkg.name.to_lowercase();
                    if !update_allowlist.contains(&pkg_name_lower) {
                        preferred.insert(pkg_name_lower, pkg.version.clone());
                    }
                }
                log::debug!("Partial update: using {} preferred versions from lock file", preferred.len());
                preferred
            }
            _ => {
                log::debug!("Full update: no preferred versions, updating all packages");
                HashMap::new()
            }
        };

        let policy = Policy::new()
            .prefer_lowest(prefer_lowest)
            .preferred_versions(preferred_versions);
        let solver = Solver::new(&pool, &policy).with_optimization(true);

        let solver_result = match solver.solve(&request) {
            Ok(result) => result,
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

        let present_packages = self.load_installed_packages();
        let transaction = Transaction::from_packages(
            present_packages,
            solver_result.packages.clone(),
            solver_result.aliases,
        );

        let packages: Vec<Package> = solver_result.packages.iter()
            .map(|p| p.as_ref().clone())
            .filter(|p| !is_platform_package(&p.name))
            .collect();

        let summary = transaction.summary();
        let lock_file_changed = summary.installs > 0 || summary.updates > 0 || summary.uninstalls > 0;

        let non_dev_roots: HashSet<String> = composer_json.require.keys()
            .filter(|k| !is_platform_package(k))
            .map(|k| k.to_lowercase())
            .collect();

        let non_dev_packages = find_transitive_dependencies(&packages, &non_dev_roots);
        let (prod_packages, dev_packages): (Vec<_>, Vec<_>) = packages.iter()
            .partition(|p| non_dev_packages.contains(&p.name.to_lowercase()));

        let install_count = packages.len();
        let update_count = 0; // TODO: track updates vs installs properly
        let removal_count = 0; // TODO: track removals
        log::info!("Lock file operations: {} installs, {} updates, {} removals",
            install_count, update_count, removal_count);

        // Extract platform requirements while preserving order from composer.json
        let platform_reqs: IndexMap<String, String> = composer_json.require.iter()
            .filter(|(name, _)| is_platform_package(name))
            .map(|(name, constraint)| (name.clone(), constraint.clone()))
            .collect();

        let platform_dev_reqs: IndexMap<String, String> = composer_json.require_dev.iter()
            .filter(|(name, _)| is_platform_package(name))
            .map(|(name, constraint)| (name.clone(), constraint.clone()))
            .collect();

        let lock = ComposerLock {
            content_hash: crate::util::compute_content_hash(&serde_json::to_string(composer_json).unwrap_or_default()),
            packages: prod_packages.iter().map(|p| LockedPackage::from(*p)).collect(),
            packages_dev: dev_packages.iter().map(|p| LockedPackage::from(*p)).collect(),
            minimum_stability: composer_json.minimum_stability.clone().unwrap_or_else(|| "stable".to_string()),
            prefer_stable: composer_json.prefer_stable.unwrap_or(false),
            prefer_lowest,
            platform: platform_reqs,
            platform_dev: platform_dev_reqs,
            plugin_api_version: "2.9.0".to_string(),
            ..Default::default()
        };

        // Only write lock file if there were changes
        if lock_file_changed && !dry_run {
            log::debug!("Writing lock file");
            let mut lock_content = serde_json::to_string_pretty(&lock).context("Failed to serialize composer.lock")?;
            // Add trailing newline to match Composer's format
            lock_content.push('\n');
            std::fs::write(working_dir.join("composer.lock"), lock_content).context("Failed to write composer.lock")?;
        }

        if update_lock_only {
             spinner.finish_and_clear();
             if lock_file_changed {
                 println!("{} Lock file updated", style("Success:").green().bold());
             } else {
                 println!("{} Lock file is up to date", style("Info:").cyan());
             }
             return Ok(0);
        }

        log::debug!("Installing dependencies from lock file");
        log::info!("Package operations: {} installs, {} updates, {} removals",
            install_count, update_count, removal_count);

        let manager = &self.composer.installation_manager;
        let result = manager.install_packages(&packages).await
            .map_err(|e| anyhow::anyhow!("Failed to install packages: {}", e))?;

        spinner.finish_and_clear();

        let actually_installed: Vec<_> = result.installed.iter()
            .filter(|p| !is_platform_package(&p.name))
            .collect();

        for pkg in &actually_installed {
            log::debug!("Installed {} ({})", pkg.name, pkg.version);
            println!("  {} {} ({})", style("-").green(), style(&pkg.name).white().bold(), style(&pkg.version).yellow());
        }

        if !dry_run {
             println!("{} Generating autoload files", style("Info:").cyan());
             
             let aliases_map: HashMap<String, Vec<String>> = HashMap::new();
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

        let total_changed = actually_installed.len() + result.updated.len();
        if total_changed > 0 || lock_file_changed {
            println!("{} {} packages updated", style("Success:").green().bold(), total_changed);
        } else {
            println!("{} Nothing to update.", style("Info:").cyan());
        }

        if !dry_run {
            self.audit_abandoned_packages(&packages);
        }

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

        if !dry_run {
            self.audit_abandoned_packages(&packages);
        }

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

    /// Load currently installed packages from composer.lock
    fn load_installed_packages(&self) -> Vec<Arc<Package>> {
        let Some(lock) = &self.composer.composer_lock else {
            return Vec::new();
        };

        let no_dev = self.composer.installation_manager.config().no_dev;

        let mut packages: Vec<Arc<Package>> = lock.packages.iter()
            .map(|lp| Arc::new(Package::from(lp)))
            .collect();

        if !no_dev {
            packages.extend(lock.packages_dev.iter().map(|lp| Arc::new(Package::from(lp))));
        }

        packages
    }

    fn audit_abandoned_packages(&self, packages: &[Package]) {
        let mut abandoned_packages: Vec<_> = packages
            .iter()
            .filter(|p| p.is_abandoned() && !p.is_platform_package())
            .collect();

        if abandoned_packages.is_empty() {
            return;
        }

        abandoned_packages.sort_by(|a, b| a.name.cmp(&b.name));

        eprintln!();
        for pkg in abandoned_packages {
            if let Some(ref abandoned) = pkg.abandoned {
                let replacement = match abandoned.replacement() {
                    Some(repl) => format!("Use {} instead", repl),
                    None => "No replacement was suggested".to_string(),
                };
                eprintln!(
                    "{} Package {} is abandoned, you should avoid using it. {}.",
                    style("Warning:").yellow(),
                    pkg.name,
                    replacement
                );
            }
        }
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

