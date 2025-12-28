//! Package manager subcommands.

pub mod bin;
mod exec;
mod dump_autoload;
mod clear_cache;
pub mod run;
pub mod platform;
mod why;

use clap::Subcommand;
use anyhow::Result;

pub use bin::BinArgs;
pub use exec::ExecArgs;
pub use dump_autoload::DumpAutoloadArgs;
pub use clear_cache::ClearCacheArgs;
pub use run::RunArgs;
pub use why::WhyArgs;

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
    }
}
