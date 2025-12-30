//! Package manager subcommands.

pub mod bin;
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

use clap::Subcommand;
use anyhow::Result;

pub use bin::BinArgs;
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

// Re-export args for pm subcommand aliases
pub use crate::install::InstallArgs;
pub use crate::update::UpdateArgs;
pub use crate::add::AddArgs;
pub use crate::remove::RemoveArgs;

/// Package manager subcommands
#[derive(Subcommand, Debug)]
pub enum PmCommands {
    /// Run a command in a bin namespace (vendor-bin plugin)
    Bin(BinArgs),

    /// Execute a vendored binary/script
    Exec(ExecArgs),

    /// Regenerate the autoloader
    #[command(name = "dump-autoload", alias = "dumpautoload")]
    DumpAutoload(DumpAutoloadArgs),

    /// Clear the Composer cache
    #[command(name = "clear-cache", alias = "clearcache")]
    ClearCache(ClearCacheArgs),

    #[command(alias = "depends")]
    Why(WhyArgs),

    #[command(name = "why-not", alias = "prohibits")]
    WhyNot(WhyArgs),

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

    /// Opens the package's repository URL or homepage in your browser
    #[command(alias = "home")]
    Browse(HomeArgs),

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
}

/// Execute a package manager command
pub async fn execute(command: PmCommands) -> Result<i32> {
    match command {
        PmCommands::Bin(args) => bin::execute(args).await,
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
        PmCommands::Browse(args) => home::execute(args).await,
        PmCommands::Install(args) => crate::install::execute(args).await,
        PmCommands::Update(args) => crate::update::execute(args).await,
        PmCommands::Add(args) => crate::add::execute(args).await,
        PmCommands::Remove(args) => crate::remove::execute(args).await,
    }
}
