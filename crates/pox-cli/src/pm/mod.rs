//! Package manager subcommands.

pub mod bin;
mod bump;
mod exec;
mod dump_autoload;
mod clear_cache;
pub mod run;
pub mod platform;
mod why;
mod show;
mod search;
mod outdated;
pub mod audit;
mod licenses;
mod home;
mod suggests;
mod fund;
mod reinstall;
pub mod recipes;

use clap::Subcommand;
use anyhow::Result;

pub use bin::BinArgs;
pub use bump::BumpArgs;
pub use exec::ExecArgs;
pub use dump_autoload::DumpAutoloadArgs;
pub use clear_cache::ClearCacheArgs;
pub use run::RunArgs;
pub use why::WhyArgs;
pub use show::ShowArgs;
pub use search::SearchArgs;
pub use outdated::OutdatedArgs;
pub use audit::AuditArgs;
pub use licenses::LicensesArgs;
pub use home::HomeArgs;
pub use suggests::SuggestsArgs;
pub use fund::FundArgs;
pub use reinstall::ReinstallArgs;
pub use recipes::{RecipesArgs, RecipesInstallArgs, RecipesUpdateArgs};

// Re-export args for pm subcommand aliases
pub use crate::install::InstallArgs;
pub use crate::update::UpdateArgs;
pub use crate::add::AddArgs;
pub use crate::remove::RemoveArgs;
pub use crate::create_project::CreateProjectArgs;

/// Package manager subcommands
#[derive(Subcommand, Debug)]
pub enum PmCommands {
    /// Run a command in a bin namespace (vendor-bin plugin)
    Bin(BinArgs),

    /// Increases the lower limit of your composer.json requirements to the currently installed versions
    Bump(BumpArgs),

    /// Execute a vendored binary/script
    Exec(ExecArgs),

    /// Regenerate the autoloader
    #[command(name = "dump-autoload", alias = "dumpautoload")]
    DumpAutoload(DumpAutoloadArgs),

    /// Clear the Composer cache
    #[command(name = "clear-cache", alias = "clearcache")]
    ClearCache(ClearCacheArgs),

    /// Shows which packages cause the given package to be installed
    #[command(alias = "depends")]
    Why(WhyArgs),

    /// Shows which packages prevent the given package from being installed
    #[command(name = "why-not", alias = "prohibits")]
    WhyNot(WhyArgs),

    /// Shows information about packages
    #[command(alias = "info")]
    Show(ShowArgs),

    /// Search for packages
    Search(SearchArgs),

    /// Shows a list of installed packages that have updates available
    Outdated(OutdatedArgs),

    /// Check for security vulnerabilities in dependencies
    Audit(AuditArgs),

    /// Shows information about licenses of dependencies
    Licenses(LicensesArgs),

    /// Discover how to help fund the maintenance of your dependencies
    Fund(FundArgs),

    /// Opens the package's repository URL or homepage in your browser
    #[command(alias = "home")]
    Browse(HomeArgs),

    /// Shows package suggestions
    #[command(alias = "suggest")]
    Suggests(SuggestsArgs),

    /// Uninstall and reinstall packages
    Reinstall(ReinstallArgs),

    /// Show Symfony recipe status for installed packages
    Recipes(RecipesArgs),

    /// Install Symfony recipes for packages
    #[command(name = "recipes:install")]
    RecipesInstall(RecipesInstallArgs),

    /// Update Symfony recipes to latest versions
    #[command(name = "recipes:update")]
    RecipesUpdate(RecipesUpdateArgs),

    /// Install project dependencies from composer.lock (alias for top-level install)
    #[command(alias = "i")]
    Install(InstallArgs),

    /// Update dependencies to their latest versions (alias for top-level update)
    Update(UpdateArgs),

    /// Add a package to the project (alias for top-level add)
    #[command(alias = "require")]
    Add(AddArgs),

    /// Remove a package from the project (alias for top-level remove)
    #[command(alias = "rm")]
    Remove(RemoveArgs),

    /// Create a new project from a package into a directory
    CreateProject(CreateProjectArgs),
}

/// Execute a package manager command
pub async fn execute(command: PmCommands) -> Result<i32> {
    match command {
        PmCommands::Bin(args) => bin::execute(args).await,
        PmCommands::Bump(args) => bump::execute(args).await,
        PmCommands::Exec(args) => exec::execute(args).await,
        PmCommands::DumpAutoload(args) => dump_autoload::execute(args).await,
        PmCommands::ClearCache(args) => clear_cache::execute(args).await,
        PmCommands::Why(args) => why::execute(args, false).await,
        PmCommands::WhyNot(args) => why::execute(args, true).await,
        PmCommands::Show(args) => show::execute(args).await,
        PmCommands::Search(args) => search::execute(args).await,
        PmCommands::Outdated(args) => outdated::execute(args).await,
        PmCommands::Audit(args) => audit::execute(args).await,
        PmCommands::Licenses(args) => licenses::execute(args).await,
        PmCommands::Fund(args) => fund::execute(args).await,
        PmCommands::Browse(args) => home::execute(args).await,
        PmCommands::Suggests(args) => suggests::execute(args).await,
        PmCommands::Reinstall(args) => reinstall::execute(args).await,
        PmCommands::Recipes(args) => recipes::execute(args).await,
        PmCommands::RecipesInstall(args) => recipes::execute_install(args).await,
        PmCommands::RecipesUpdate(args) => recipes::execute_update(args).await,
        PmCommands::Install(args) => crate::install::execute(args).await,
        PmCommands::Update(args) => crate::update::execute(args).await,
        PmCommands::Add(args) => crate::add::execute(args).await,
        PmCommands::Remove(args) => crate::remove::execute(args).await,
        PmCommands::CreateProject(args) => crate::create_project::execute(args).await,
    }
}
