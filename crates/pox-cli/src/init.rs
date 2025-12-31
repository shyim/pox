//! Init command - create a new composer.json file with interactive prompts.

use anyhow::{Context, Result};
use clap::Args;
use console::style;
use dialoguer::{Confirm, Input};
use regex::Regex;
use pox_spdx::SpdxLicenses;
use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::Command;

#[derive(Args, Debug)]
pub struct InitArgs {
    /// Package name (vendor/package)
    #[arg(long)]
    pub name: Option<String>,

    /// Package description
    #[arg(long)]
    pub description: Option<String>,

    /// Author in format "Name <email>"
    #[arg(long)]
    pub author: Option<String>,

    /// Package type (e.g., library, project, metapackage, composer-plugin)
    #[arg(long, name = "type")]
    pub package_type: Option<String>,

    /// Package homepage
    #[arg(long)]
    pub homepage: Option<String>,

    /// Require a package (can be used multiple times)
    #[arg(long, action = clap::ArgAction::Append)]
    pub require: Vec<String>,

    /// Require a dev package (can be used multiple times)
    #[arg(long, action = clap::ArgAction::Append)]
    pub require_dev: Vec<String>,

    /// Minimum stability (stable, RC, beta, alpha, dev)
    #[arg(long, short = 's')]
    pub stability: Option<String>,

    /// License (SPDX identifier)
    #[arg(long, short = 'l')]
    pub license: Option<String>,

    /// Add PSR-4 autoload mapping (e.g., src/)
    #[arg(long, short = 'a')]
    pub autoload: Option<String>,

    /// Add custom repositories (URL or JSON)
    #[arg(long, action = clap::ArgAction::Append)]
    pub repository: Vec<String>,

    /// Working directory
    #[arg(short = 'd', long, default_value = ".")]
    pub working_dir: PathBuf,

    /// Non-interactive mode (use defaults and provided options)
    #[arg(long, short = 'n')]
    pub no_interaction: bool,
}

/// Git configuration values
struct GitConfig {
    user_name: Option<String>,
    user_email: Option<String>,
    github_user: Option<String>,
}

impl GitConfig {
    fn load() -> Self {
        let user_name = get_git_config("user.name");
        let user_email = get_git_config("user.email");
        let github_user = get_git_config("github.user");

        Self {
            user_name,
            user_email,
            github_user,
        }
    }

    fn default_author(&self) -> Option<String> {
        match (&self.user_name, &self.user_email) {
            (Some(name), Some(email)) => Some(format!("{} <{}>", name, email)),
            (Some(name), None) => Some(name.clone()),
            _ => None,
        }
    }

    fn default_vendor(&self) -> Option<String> {
        self.github_user.clone()
    }
}

fn get_git_config(key: &str) -> Option<String> {
    Command::new("git")
        .args(["config", "--get", key])
        .output()
        .ok()
        .and_then(|output| {
            if output.status.success() {
                String::from_utf8(output.stdout)
                    .ok()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
            } else {
                None
            }
        })
}

fn is_git_repo(path: &PathBuf) -> bool {
    path.join(".git").exists()
}

/// Validate package name format: vendor/package (lowercase, alphanumeric with _.- allowed)
fn validate_package_name(name: &str) -> Result<(), String> {
    let re = Regex::new(r"^[a-z0-9]([_.-]?[a-z0-9]+)*/[a-z0-9](([_.]|-{1,2})?[a-z0-9]+)*$").unwrap();
    if re.is_match(name) {
        Ok(())
    } else {
        Err(format!(
            "The package name '{}' is invalid. It should be lowercase and have a vendor name, \
             a forward slash, and a package name, matching: [a-z0-9_.-]+/[a-z0-9_.-]+",
            name
        ))
    }
}

/// Validate author format: Name or Name <email>
fn validate_author(author: &str) -> Result<(), String> {
    if author.is_empty() {
        return Err("Author cannot be empty".to_string());
    }

    // Check for email format if present
    if author.contains('<') {
        let re = Regex::new(r"^[^<>]+\s+<[^<>]+@[^<>]+>$").unwrap();
        if !re.is_match(author) {
            return Err("Invalid author format. Expected: Name or Name <email@example.com>".to_string());
        }
    }

    Ok(())
}

/// Validate minimum stability
fn validate_stability(stability: &str) -> Result<(), String> {
    let valid = ["stable", "rc", "beta", "alpha", "dev"];
    if valid.contains(&stability.to_lowercase().as_str()) {
        Ok(())
    } else {
        Err(format!(
            "Invalid minimum stability '{}'. Must be one of: {}",
            stability,
            valid.join(", ")
        ))
    }
}

/// Validate license using SPDX
fn validate_license(license: &str) -> Result<(), String> {
    if license.to_lowercase() == "proprietary" {
        return Ok(());
    }

    let spdx = SpdxLicenses::new();
    if spdx.validate(license) {
        Ok(())
    } else {
        Err(format!(
            "Invalid license '{}'. Only SPDX license identifiers (https://spdx.org/licenses/) \
             or 'proprietary' are accepted.",
            license
        ))
    }
}

/// Derive namespace from package name (vendor/package -> Vendor\Package)
fn namespace_from_package_name(package_name: &str) -> Option<String> {
    if !package_name.contains('/') {
        return None;
    }

    let namespace: Vec<String> = package_name
        .split('/')
        .map(|part| {
            // Replace non-alphanumeric with space, title case, remove spaces
            let cleaned: String = part
                .chars()
                .map(|c| if c.is_alphanumeric() { c } else { ' ' })
                .collect();

            cleaned
                .split_whitespace()
                .map(|word| {
                    let mut chars = word.chars();
                    match chars.next() {
                        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                        None => String::new(),
                    }
                })
                .collect::<String>()
        })
        .collect();

    Some(namespace.join("\\"))
}

/// Sanitize a string to be used as part of package name
fn sanitize_package_component(name: &str) -> String {
    // Convert CamelCase to kebab-case
    let mut result = String::new();
    for (i, c) in name.chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            result.push('-');
        }
        result.push(c.to_lowercase().next().unwrap_or(c));
    }

    // Remove invalid characters and clean up
    let re = Regex::new(r"[^a-z0-9_.-]").unwrap();
    let cleaned = re.replace_all(&result, "").to_string();

    // Remove leading/trailing separators and collapse multiple separators
    let re2 = Regex::new(r"^[_.-]+|[_.-]+$").unwrap();
    let cleaned = re2.replace_all(&cleaned, "").to_string();

    let re3 = Regex::new(r"([_.-]){2,}").unwrap();
    re3.replace_all(&cleaned, "$1").to_string()
}

/// Check if .gitignore has vendor ignored
fn has_vendor_ignore(gitignore_path: &PathBuf) -> bool {
    if !gitignore_path.exists() {
        return false;
    }

    if let Ok(content) = std::fs::read_to_string(gitignore_path) {
        let re = Regex::new(r"(?m)^/?vendor(/\*?)?$").unwrap();
        return re.is_match(&content);
    }

    false
}

/// Add vendor to .gitignore
fn add_vendor_to_gitignore(gitignore_path: &PathBuf) -> Result<()> {
    let mut content = if gitignore_path.exists() {
        std::fs::read_to_string(gitignore_path)?
    } else {
        String::new()
    };

    if !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }

    content.push_str("/vendor/\n");
    std::fs::write(gitignore_path, content)?;

    Ok(())
}

pub async fn execute(args: InitArgs) -> Result<i32> {
    let working_dir = args
        .working_dir
        .canonicalize()
        .context("Failed to resolve working directory")?;

    let json_path = working_dir.join("composer.json");

    if json_path.exists() {
        eprintln!(
            "{} composer.json already exists in {}",
            style("Error:").red().bold(),
            working_dir.display()
        );
        return Ok(1);
    }

    let git_config = GitConfig::load();
    let is_interactive = !args.no_interaction && std::io::stdin().is_terminal();

    println!(
        "\n{}",
        style("Welcome to the Composer config generator").cyan().bold()
    );
    println!("\nThis command will guide you through creating your composer.json config.\n");

    // Collect all configuration values
    let mut json_obj = serde_json::Map::new();

    // --- Package Name ---
    let default_name = {
        let dir_name = working_dir
            .file_name()
            .map(|n| sanitize_package_component(&n.to_string_lossy()))
            .unwrap_or_else(|| "project".to_string());

        let vendor = git_config
            .default_vendor()
            .map(|v| sanitize_package_component(&v))
            .or_else(|| std::env::var("USER").ok().map(|u| sanitize_package_component(&u)))
            .unwrap_or_else(|| "vendor".to_string());

        format!("{}/{}", vendor, dir_name)
    };

    let name = if let Some(name) = args.name {
        validate_package_name(&name).map_err(|e| anyhow::anyhow!(e))?;
        name
    } else if is_interactive {
        Input::<String>::new()
            .with_prompt("Package name (<vendor>/<name>)")
            .default(default_name)
            .validate_with(|input: &String| validate_package_name(input))
            .interact_text()?
    } else {
        default_name
    };
    json_obj.insert("name".to_string(), serde_json::Value::String(name.clone()));

    // --- Description ---
    let description = if let Some(desc) = args.description {
        Some(desc)
    } else if is_interactive {
        let desc: String = Input::new()
            .with_prompt("Description")
            .allow_empty(true)
            .interact_text()?;
        if desc.is_empty() {
            None
        } else {
            Some(desc)
        }
    } else {
        None
    };
    if let Some(desc) = description {
        json_obj.insert("description".to_string(), serde_json::Value::String(desc));
    }

    // --- Author ---
    let author = if let Some(author) = args.author {
        validate_author(&author).map_err(|e| anyhow::anyhow!(e))?;
        Some(author)
    } else if is_interactive {
        let default_author = git_config.default_author();
        let prompt = if let Some(ref def) = default_author {
            format!("Author [{}], n to skip", def)
        } else {
            "Author (n to skip)".to_string()
        };

        let author_input: String = Input::new()
            .with_prompt(&prompt)
            .default(default_author.unwrap_or_default())
            .allow_empty(true)
            .interact_text()?;

        if author_input.is_empty() || author_input == "n" || author_input == "no" {
            None
        } else {
            validate_author(&author_input).map_err(|e| anyhow::anyhow!(e))?;
            Some(author_input)
        }
    } else {
        git_config.default_author()
    };

    if let Some(author) = author {
        // Parse author into name and email
        let authors = parse_author(&author);
        json_obj.insert("authors".to_string(), serde_json::Value::Array(authors));
    }

    // --- Minimum Stability ---
    let stability = if let Some(stab) = args.stability {
        validate_stability(&stab).map_err(|e| anyhow::anyhow!(e))?;
        Some(stab.to_lowercase())
    } else if is_interactive {
        let stab: String = Input::new()
            .with_prompt("Minimum Stability (stable, RC, beta, alpha, dev)")
            .allow_empty(true)
            .validate_with(|input: &String| {
                if input.is_empty() {
                    Ok(())
                } else {
                    validate_stability(input)
                }
            })
            .interact_text()?;

        if stab.is_empty() {
            None
        } else {
            Some(stab.to_lowercase())
        }
    } else {
        None
    };
    if let Some(stab) = stability {
        json_obj.insert(
            "minimum-stability".to_string(),
            serde_json::Value::String(stab),
        );
    }

    // --- Package Type ---
    let pkg_type = if let Some(t) = args.package_type {
        Some(t)
    } else if is_interactive {
        let t: String = Input::new()
            .with_prompt("Package Type (e.g. library, project, metapackage, composer-plugin)")
            .allow_empty(true)
            .interact_text()?;

        if t.is_empty() {
            None
        } else {
            Some(t)
        }
    } else {
        None
    };
    if let Some(t) = pkg_type {
        json_obj.insert("type".to_string(), serde_json::Value::String(t));
    }

    // --- License ---
    let license = if let Some(lic) = args.license {
        validate_license(&lic).map_err(|e| anyhow::anyhow!(e))?;
        Some(lic)
    } else if is_interactive {
        let lic: String = Input::new()
            .with_prompt("License (SPDX identifier, e.g. MIT, GPL-3.0-or-later)")
            .allow_empty(true)
            .validate_with(|input: &String| {
                if input.is_empty() {
                    Ok(())
                } else {
                    validate_license(input)
                }
            })
            .interact_text()?;

        if lic.is_empty() {
            None
        } else {
            Some(lic)
        }
    } else {
        None
    };
    if let Some(lic) = license {
        json_obj.insert("license".to_string(), serde_json::Value::String(lic));
    }

    // --- Autoload ---
    let autoload_path = if let Some(autoload) = args.autoload {
        Some(autoload)
    } else if is_interactive {
        let namespace = namespace_from_package_name(&name);
        let prompt = if let Some(ref ns) = namespace {
            format!(
                "Add PSR-4 autoload mapping? Maps namespace \"{}\" to the entered relative path (n to skip)",
                ns
            )
        } else {
            "Add PSR-4 autoload mapping? Enter relative path (n to skip)".to_string()
        };

        let autoload: String = Input::new()
            .with_prompt(&prompt)
            .default("src/".to_string())
            .interact_text()?;

        if autoload == "n" || autoload == "no" || autoload.is_empty() {
            None
        } else {
            Some(autoload)
        }
    } else {
        None
    };

    if let Some(ref path) = autoload_path {
        if let Some(namespace) = namespace_from_package_name(&name) {
            let mut psr4 = serde_json::Map::new();
            psr4.insert(
                format!("{}\\", namespace),
                serde_json::Value::String(path.clone()),
            );
            let mut autoload = serde_json::Map::new();
            autoload.insert("psr-4".to_string(), serde_json::Value::Object(psr4));
            json_obj.insert("autoload".to_string(), serde_json::Value::Object(autoload));
        }
    }

    // --- Repositories ---
    if !args.repository.is_empty() {
        let repos: Vec<serde_json::Value> = args
            .repository
            .iter()
            .filter_map(|repo| {
                // Try to parse as JSON first
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(repo) {
                    Some(json)
                } else {
                    // Treat as URL
                    let mut obj = serde_json::Map::new();
                    obj.insert("type".to_string(), serde_json::Value::String("composer".to_string()));
                    obj.insert("url".to_string(), serde_json::Value::String(repo.clone()));
                    Some(serde_json::Value::Object(obj))
                }
            })
            .collect();

        if !repos.is_empty() {
            json_obj.insert("repositories".to_string(), serde_json::Value::Array(repos));
        }
    }

    // --- Require ---
    if !args.require.is_empty() {
        let mut require_map = serde_json::Map::new();
        for spec in &args.require {
            let (pkg_name, constraint) = parse_package_spec(spec);
            require_map.insert(pkg_name, serde_json::Value::String(constraint));
        }
        json_obj.insert("require".to_string(), serde_json::Value::Object(require_map));
    } else {
        // Empty require object
        json_obj.insert(
            "require".to_string(),
            serde_json::Value::Object(serde_json::Map::new()),
        );
    }

    // --- Require-dev ---
    if !args.require_dev.is_empty() {
        let mut require_dev_map = serde_json::Map::new();
        for spec in &args.require_dev {
            let (pkg_name, constraint) = parse_package_spec(spec);
            require_dev_map.insert(pkg_name, serde_json::Value::String(constraint));
        }
        json_obj.insert(
            "require-dev".to_string(),
            serde_json::Value::Object(require_dev_map),
        );
    }

    // --- Show confirmation ---
    let json_content =
        serde_json::to_string_pretty(&serde_json::Value::Object(json_obj.clone()))?;

    println!("\n{}", json_content);

    if is_interactive {
        let confirm = Confirm::new()
            .with_prompt("Do you confirm generation?")
            .default(true)
            .interact()?;

        if !confirm {
            println!("{}", style("Command aborted").red());
            return Ok(1);
        }
    }

    // --- Write composer.json ---
    std::fs::write(&json_path, &json_content).context("Failed to write composer.json")?;

    println!(
        "\n{} Created {}",
        style("Success:").green().bold(),
        json_path.display()
    );

    // --- Create autoload directory ---
    if let Some(ref path) = autoload_path {
        let autoload_dir = working_dir.join(path);
        if !autoload_dir.exists() {
            std::fs::create_dir_all(&autoload_dir)
                .context(format!("Failed to create directory: {}", autoload_dir.display()))?;
            println!(
                "  {} Created {}",
                style("✓").green(),
                autoload_dir.display()
            );
        }
    }

    // --- Handle .gitignore ---
    if is_git_repo(&working_dir) {
        let gitignore_path = working_dir.join(".gitignore");

        if !has_vendor_ignore(&gitignore_path) {
            let add_ignore = if is_interactive {
                Confirm::new()
                    .with_prompt("Would you like the vendor directory added to your .gitignore?")
                    .default(true)
                    .interact()?
            } else {
                true // Add by default in non-interactive mode
            };

            if add_ignore {
                add_vendor_to_gitignore(&gitignore_path)?;
                println!(
                    "  {} Added /vendor/ to .gitignore",
                    style("✓").green()
                );
            }
        }
    }

    // --- Show autoload info ---
    if let Some(ref path) = autoload_path {
        if let Some(namespace) = namespace_from_package_name(&name) {
            println!(
                "\n{} PSR-4 autoloading configured. Use \"{}\" in {}",
                style("Info:").cyan(),
                style(format!("namespace {};", namespace)).yellow(),
                path
            );
            println!(
                "      Include the Composer autoloader with: {}",
                style("require 'vendor/autoload.php';").yellow()
            );
        }
    }

    Ok(0)
}

/// Parse a package specification (vendor/package:^1.0 or vendor/package)
fn parse_package_spec(spec: &str) -> (String, String) {
    // Try different separators: :, =, or space
    if let Some(pos) = spec.find(':') {
        let name = spec[..pos].to_string();
        let constraint = spec[pos + 1..].to_string();
        (name, constraint)
    } else if let Some(pos) = spec.find('=') {
        let name = spec[..pos].to_string();
        let constraint = spec[pos + 1..].to_string();
        (name, constraint)
    } else if let Some(pos) = spec.find(' ') {
        let name = spec[..pos].to_string();
        let constraint = spec[pos + 1..].to_string();
        (name, constraint)
    } else {
        (spec.to_string(), "*".to_string())
    }
}

/// Parse author string into JSON array of author objects
fn parse_author(author: &str) -> Vec<serde_json::Value> {
    let re = Regex::new(r"^(?P<name>.+?)\s*<(?P<email>[^>]+)>$").unwrap();

    let author_obj = if let Some(caps) = re.captures(author) {
        let mut obj = serde_json::Map::new();
        obj.insert(
            "name".to_string(),
            serde_json::Value::String(caps["name"].trim().to_string()),
        );
        obj.insert(
            "email".to_string(),
            serde_json::Value::String(caps["email"].to_string()),
        );
        serde_json::Value::Object(obj)
    } else {
        let mut obj = serde_json::Map::new();
        obj.insert(
            "name".to_string(),
            serde_json::Value::String(author.trim().to_string()),
        );
        serde_json::Value::Object(obj)
    };

    vec![author_obj]
}
