//! Symfony Flex recipes command - show and manage recipes.
//!
//! This command shows the status of installed recipes for Symfony packages.
//! Similar to `composer recipes` provided by symfony/flex.

use anyhow::{Context, Result};
use clap::Args;
use console::style;
use std::path::PathBuf;
use pox_pm::{
    json::{ComposerJson, ComposerLock, LockedPackage},
    plugin::{FlexConfig, FlexLock},
};

#[derive(Args, Debug)]
pub struct RecipesArgs {
    /// Show only outdated recipes
    #[arg(short = 'o', long)]
    pub outdated: bool,

    /// Output format: text or json
    #[arg(short = 'f', long, default_value = "text")]
    pub format: String,

    /// Working directory
    #[arg(short = 'd', long, default_value = ".")]
    pub working_dir: PathBuf,
}

#[derive(Args, Debug)]
pub struct RecipesInstallArgs {
    /// Package name to install recipes for (or all if not specified)
    pub package: Option<String>,

    /// Force reinstall of recipes
    #[arg(long)]
    pub force: bool,

    /// Working directory
    #[arg(short = 'd', long, default_value = ".")]
    pub working_dir: PathBuf,
}

#[derive(Args, Debug)]
pub struct RecipesUpdateArgs {
    /// Package name to update recipes for (or all if not specified)
    pub package: Option<String>,

    /// Working directory
    #[arg(short = 'd', long, default_value = ".")]
    pub working_dir: PathBuf,
}

/// Execute the recipes command - show recipe status
pub async fn execute(args: RecipesArgs) -> Result<i32> {
    let working_dir = args
        .working_dir
        .canonicalize()
        .context("Failed to resolve working directory")?;

    let lock_path = working_dir.join("symfony.lock");

    // Check if symfony/flex is installed
    let json_path = working_dir.join("composer.json");
    let composer_json: ComposerJson = if json_path.exists() {
        let content = std::fs::read_to_string(&json_path)?;
        serde_json::from_str(&content)?
    } else {
        eprintln!("Error: No composer.json found in {}", working_dir.display());
        return Ok(1);
    };

    let has_flex = composer_json.require.contains_key("symfony/flex")
        || composer_json.require_dev.contains_key("symfony/flex");

    if !has_flex {
        eprintln!("Error: symfony/flex is not installed. Install it first with:");
        eprintln!("  pox add symfony/flex");
        return Ok(1);
    }

    // Load symfony.lock
    let lock = FlexLock::load(&lock_path)?;

    // Load composer.lock to get installed packages
    let composer_lock_path = working_dir.join("composer.lock");
    let composer_lock: Option<ComposerLock> = if composer_lock_path.exists() {
        let content = std::fs::read_to_string(&composer_lock_path)?;
        serde_json::from_str(&content).ok()
    } else {
        None
    };

    let Some(composer_lock) = composer_lock else {
        eprintln!("Error: No composer.lock found. Run 'pox install' first.");
        return Ok(1);
    };

    // Collect installed packages
    let installed_packages: Vec<_> = composer_lock
        .packages
        .iter()
        .chain(composer_lock.packages_dev.iter())
        .collect();

    if installed_packages.is_empty() {
        println!("No packages installed.");
        return Ok(0);
    }

    // Load flex config for endpoints
    let flex_config = FlexConfig::from_composer_json(&composer_json);

    if args.format == "json" {
        print_recipes_json(&installed_packages, &lock)?;
    } else {
        print_recipes_text(&installed_packages, &lock, &flex_config, args.outdated)?;
    }

    Ok(0)
}

fn print_recipes_text(
    packages: &[&LockedPackage],
    lock: &FlexLock,
    _config: &FlexConfig,
    outdated_only: bool,
) -> Result<()> {
    let mut packages_with_recipes = Vec::new();
    let mut packages_without_recipes = Vec::new();

    for pkg in packages {
        if lock.has(&pkg.name) {
            packages_with_recipes.push(pkg);
        } else {
            // Check if it's a symfony package (might have a recipe available)
            if pkg.name.starts_with("symfony/") {
                packages_without_recipes.push(pkg);
            }
        }
    }

    if !outdated_only && packages_with_recipes.is_empty() && packages_without_recipes.is_empty() {
        println!("No Symfony recipes found for installed packages.");
        return Ok(());
    }

    if !packages_with_recipes.is_empty() {
        println!("{}", style("Configured recipes:").bold().green());
        println!();

        for pkg in &packages_with_recipes {
            let lock_data = lock.get(&pkg.name);
            let _version = lock_data
                .and_then(|d| d.get("version"))
                .and_then(|v| v.as_str())
                .unwrap_or("auto");

            let recipe_info = lock_data
                .and_then(|d| d.get("recipe"))
                .and_then(|r| r.get("version"))
                .and_then(|v| v.as_str())
                .map(|v| format!(" (recipe: {})", v))
                .unwrap_or_default();

            println!(
                "  {} {} {}{}",
                style("•").green(),
                style(&pkg.name).bold(),
                pkg.version,
                style(recipe_info).dim()
            );
        }
    }

    if !outdated_only && !packages_without_recipes.is_empty() {
        if !packages_with_recipes.is_empty() {
            println!();
        }
        println!("{}", style("Packages that may have recipes available:").bold().yellow());
        println!();

        for pkg in &packages_without_recipes {
            println!(
                "  {} {} {}",
                style("○").yellow(),
                style(&pkg.name).bold(),
                pkg.version
            );
        }

        println!();
        println!("Run {} to install recipes for these packages.", style("pox pm recipes:install").bold());
    }

    Ok(())
}

fn print_recipes_json(
    packages: &[&LockedPackage],
    lock: &FlexLock,
) -> Result<()> {
    let mut recipes = Vec::new();

    for pkg in packages {
        if let Some(lock_data) = lock.get(&pkg.name) {
            recipes.push(serde_json::json!({
                "name": pkg.name,
                "version": pkg.version,
                "recipe": lock_data,
            }));
        }
    }

    println!("{}", serde_json::to_string_pretty(&recipes)?);
    Ok(())
}

/// Execute the recipes:install command - install recipes for packages
pub async fn execute_install(args: RecipesInstallArgs) -> Result<i32> {
    let working_dir = args
        .working_dir
        .canonicalize()
        .context("Failed to resolve working directory")?;

    // Load composer.json
    let json_path = working_dir.join("composer.json");
    let composer_json: ComposerJson = if json_path.exists() {
        let content = std::fs::read_to_string(&json_path)?;
        serde_json::from_str(&content)?
    } else {
        eprintln!("Error: No composer.json found in {}", working_dir.display());
        return Ok(1);
    };

    // Check if symfony/flex is installed
    let has_flex = composer_json.require.contains_key("symfony/flex")
        || composer_json.require_dev.contains_key("symfony/flex");

    if !has_flex {
        eprintln!("Error: symfony/flex is not installed. Install it first with:");
        eprintln!("  pox add symfony/flex");
        return Ok(1);
    }

    // Load composer.lock
    let composer_lock_path = working_dir.join("composer.lock");
    let composer_lock: Option<ComposerLock> = if composer_lock_path.exists() {
        let content = std::fs::read_to_string(&composer_lock_path)?;
        serde_json::from_str(&content).ok()
    } else {
        None
    };

    let Some(composer_lock) = composer_lock else {
        eprintln!("Error: No composer.lock found. Run 'pox install' first.");
        return Ok(1);
    };

    // Load existing symfony.lock
    let lock_path = working_dir.join("symfony.lock");
    let mut lock = FlexLock::load(&lock_path)?;

    // Get packages to process
    let packages_to_process: Vec<_> = if let Some(ref package_name) = args.package {
        composer_lock
            .packages
            .iter()
            .chain(composer_lock.packages_dev.iter())
            .filter(|p| &p.name == package_name)
            .collect()
    } else {
        composer_lock
            .packages
            .iter()
            .chain(composer_lock.packages_dev.iter())
            .collect()
    };

    if packages_to_process.is_empty() {
        if let Some(ref name) = args.package {
            eprintln!("Error: Package '{}' not found in composer.lock", name);
        } else {
            println!("No packages to process.");
        }
        return Ok(0);
    }

    // Get flex config and HTTP client
    let flex_config = FlexConfig::from_composer_json(&composer_json);
    let http_client = pox_pm::http::HttpClient::new()?;

    println!("{}", style("Installing Symfony recipes...").bold());

    let mut installed_count = 0;
    let mut skipped_count = 0;

    for pkg in &packages_to_process {
        // Skip if already has recipe and not forcing
        if lock.has(&pkg.name) && !args.force {
            skipped_count += 1;
            continue;
        }

        // Try to download and install recipe
        match download_and_install_recipe(
            &working_dir,
            pkg,
            &flex_config,
            &http_client,
            &mut lock,
        )
        .await
        {
            Ok(true) => {
                println!("  {} {} recipe installed", style("✓").green(), pkg.name);
                installed_count += 1;
            }
            Ok(false) => {
                // No recipe available
            }
            Err(e) => {
                eprintln!("  {} {} failed: {}", style("✗").red(), pkg.name, e);
            }
        }
    }

    // Save lock
    lock.save(&lock_path)?;

    if installed_count > 0 || skipped_count > 0 {
        println!();
        if installed_count > 0 {
            println!("Installed {} recipe(s).", installed_count);
        }
        if skipped_count > 0 {
            println!("Skipped {} package(s) (already have recipes).", skipped_count);
        }
    } else {
        println!("No new recipes to install.");
    }

    Ok(0)
}

/// Execute the recipes:update command - update recipes to latest versions
pub async fn execute_update(args: RecipesUpdateArgs) -> Result<i32> {
    let working_dir = args
        .working_dir
        .canonicalize()
        .context("Failed to resolve working directory")?;

    // Load composer.json
    let json_path = working_dir.join("composer.json");
    let composer_json: ComposerJson = if json_path.exists() {
        let content = std::fs::read_to_string(&json_path)?;
        serde_json::from_str(&content)?
    } else {
        eprintln!("Error: No composer.json found in {}", working_dir.display());
        return Ok(1);
    };

    // Load symfony.lock
    let lock_path = working_dir.join("symfony.lock");
    let _lock = FlexLock::load(&lock_path)?;

    if args.package.is_some() {
        println!("Updating recipe for {}...", args.package.as_ref().unwrap());
    } else {
        println!("{}", style("Checking for recipe updates...").bold());
    }

    // Get flex config
    let _flex_config = FlexConfig::from_composer_json(&composer_json);

    // For now, just show what would be updated
    // Full implementation would compare recipe versions from the index

    println!();
    println!(
        "{}",
        style("Recipe updates are not yet fully implemented.").yellow()
    );
    println!("Current recipes in symfony.lock:");

    let lock_content = std::fs::read_to_string(&lock_path)?;
    let lock_data: serde_json::Value = serde_json::from_str(&lock_content)?;

    if let Some(obj) = lock_data.as_object() {
        for (name, data) in obj {
            let version = data
                .get("recipe")
                .and_then(|r| r.get("version"))
                .and_then(|v| v.as_str())
                .unwrap_or("auto");
            println!("  {} (recipe: {})", name, version);
        }
    }

    Ok(0)
}

async fn download_and_install_recipe(
    working_dir: &std::path::Path,
    package: &LockedPackage,
    config: &FlexConfig,
    http_client: &pox_pm::http::HttpClient,
    lock: &mut FlexLock,
) -> Result<bool> {
    // Try to download recipe index and find recipe for this package
    // This is a simplified version - the full implementation in symfony_flex.rs has more details

    for endpoint in &config.endpoints {
        // Download index
        let index: serde_json::Value = match http_client.get_json(endpoint).await {
            Ok(idx) => idx,
            Err(_) => continue,
        };

        // Check if package has a recipe
        let recipes = index.get("recipes").and_then(|r| r.as_object());
        if let Some(recipes) = recipes {
            if let Some(versions) = recipes.get(&package.name).and_then(|v| v.as_array()) {
                // Find best matching version
                let pkg_version = parse_version(&package.version);

                let best_version = versions
                    .iter()
                    .filter_map(|v| v.as_str())
                    .filter(|v| {
                        let recipe_version = parse_version(v);
                        !version_less_than(&pkg_version, &recipe_version)
                    })
                    .max_by(|a, b| {
                        let va = parse_version(a);
                        let vb = parse_version(b);
                        compare_versions(&va, &vb)
                    });

                if let Some(recipe_version) = best_version {
                    // Get links for building recipe URL
                    let links = index.get("_links");
                    let recipe_template = links
                        .and_then(|l| l.get("recipe_template"))
                        .and_then(|t| t.as_str());

                    let recipe_url = if let Some(template) = recipe_template {
                        template
                            .replace("{package_dotted}", &package.name.replace('/', "."))
                            .replace("{package}", &package.name)
                            .replace("{version}", recipe_version)
                    } else {
                        format!(
                            "https://raw.githubusercontent.com/symfony/recipes/flex/main/{}/{}/manifest.json",
                            package.name, recipe_version
                        )
                    };

                    // Download and apply recipe
                    let manifest: serde_json::Value = http_client.get_json(&recipe_url).await?;

                    // Apply recipe configurators
                    apply_recipe_manifest(working_dir, &package.name, &manifest, config)?;

                    // Update lock
                    lock.set(
                        &package.name,
                        serde_json::json!({
                            "version": package.version,
                            "recipe": {
                                "version": recipe_version,
                            }
                        }),
                    );

                    return Ok(true);
                }
            }
        }
    }

    Ok(false)
}

fn apply_recipe_manifest(
    working_dir: &std::path::Path,
    package_name: &str,
    manifest: &serde_json::Value,
    config: &FlexConfig,
) -> Result<()> {
    // Apply bundles
    if let Some(bundles) = manifest.get("bundles").and_then(|b| b.as_object()) {
        let bundles_file = working_dir.join(&config.config_dir).join("bundles.php");
        apply_bundles(&bundles_file, bundles)?;
        println!("    Enabling {} as a Symfony bundle", package_name);
    }

    // Apply env
    if let Some(env) = manifest.get("env").and_then(|e| e.as_object()) {
        apply_env(working_dir, package_name, env)?;
        println!("    Adding environment variables");
    }

    // Apply gitignore
    if let Some(gitignore) = manifest.get("gitignore").and_then(|g| g.as_array()) {
        apply_gitignore(working_dir, package_name, gitignore, config)?;
        println!("    Adding .gitignore entries");
    }

    Ok(())
}

fn apply_bundles(
    bundles_file: &std::path::Path,
    bundles: &serde_json::Map<String, serde_json::Value>,
) -> Result<()> {
    // Create parent dir if needed
    if let Some(parent) = bundles_file.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Load existing or create new
    let mut content = if bundles_file.exists() {
        std::fs::read_to_string(bundles_file)?
    } else {
        "<?php\n\nreturn [\n];\n".to_string()
    };

    // For each bundle, add if not present
    for (class, envs) in bundles {
        let class = class.trim_start_matches('\\');
        if content.contains(&format!("{}::class", class)) {
            continue;
        }

        // Build env string
        let env_str = if let Some(envs) = envs.as_array() {
            envs.iter()
                .filter_map(|e| e.as_str())
                .map(|e| format!("'{}' => true", e))
                .collect::<Vec<_>>()
                .join(", ")
        } else {
            "'all' => true".to_string()
        };

        // Insert before closing bracket
        let insert_line = format!("    {}::class => [{}],\n", class, env_str);
        if let Some(pos) = content.rfind("];") {
            content.insert_str(pos, &insert_line);
        }
    }

    std::fs::write(bundles_file, content)?;
    Ok(())
}

fn apply_env(
    working_dir: &std::path::Path,
    package_name: &str,
    env_vars: &serde_json::Map<String, serde_json::Value>,
) -> Result<()> {
    let dotenv_path = working_dir.join(".env");
    if !dotenv_path.exists() {
        return Ok(());
    }

    let content = std::fs::read_to_string(&dotenv_path)?;
    if content.contains(&format!("###> {} ###", package_name)) {
        return Ok(());
    }

    let mut block = format!("\n###> {} ###\n", package_name);
    for (key, value) in env_vars {
        let value_str = value.as_str().unwrap_or("");
        if key.starts_with('#') {
            block.push_str(&format!("# {}\n", value_str));
        } else {
            block.push_str(&format!("{}={}\n", key, value_str));
        }
    }
    block.push_str(&format!("###< {} ###\n", package_name));

    let mut new_content = content;
    new_content.push_str(&block);
    std::fs::write(&dotenv_path, new_content)?;

    Ok(())
}

fn apply_gitignore(
    working_dir: &std::path::Path,
    package_name: &str,
    entries: &[serde_json::Value],
    config: &FlexConfig,
) -> Result<()> {
    let gitignore_path = working_dir.join(".gitignore");

    let content = if gitignore_path.exists() {
        std::fs::read_to_string(&gitignore_path)?
    } else {
        String::new()
    };

    if content.contains(&format!("###> {} ###", package_name)) {
        return Ok(());
    }

    let mut block = format!("\n###> {} ###\n", package_name);
    for entry in entries {
        if let Some(entry_str) = entry.as_str() {
            let expanded = entry_str
                .replace("%CONFIG_DIR%", &config.config_dir)
                .replace("%SRC_DIR%", &config.src_dir)
                .replace("%VAR_DIR%", &config.var_dir)
                .replace("%PUBLIC_DIR%", &config.public_dir);
            block.push_str(&format!("{}\n", expanded));
        }
    }
    block.push_str(&format!("###< {} ###\n", package_name));

    let mut new_content = content;
    new_content.push_str(&block);
    std::fs::write(&gitignore_path, new_content)?;

    Ok(())
}

fn parse_version(version: &str) -> Vec<u32> {
    version
        .trim_start_matches("dev-")
        .trim_start_matches('v')
        .trim_end_matches(".x-dev")
        .trim_end_matches("-dev")
        .split('.')
        .filter_map(|p| p.parse::<u32>().ok())
        .collect()
}

fn version_less_than(a: &[u32], b: &[u32]) -> bool {
    compare_versions(a, b) == std::cmp::Ordering::Less
}

fn compare_versions(a: &[u32], b: &[u32]) -> std::cmp::Ordering {
    use std::cmp::Ordering;

    let max_len = a.len().max(b.len());
    for i in 0..max_len {
        let av = a.get(i).unwrap_or(&0);
        let bv = b.get(i).unwrap_or(&0);
        match av.cmp(bv) {
            Ordering::Equal => continue,
            other => return other,
        }
    }
    Ordering::Equal
}
