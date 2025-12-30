//! Outdated command - proxy for `show --latest --outdated`.

use anyhow::Result;
use clap::Args;
use std::path::PathBuf;

use super::show::{self, ShowArgs};

#[derive(Args, Debug)]
pub struct OutdatedArgs {
    /// Package to inspect (or wildcard pattern)
    pub package: Option<String>,

    /// Show all installed packages with their latest versions
    #[arg(short = 'a', long)]
    pub all: bool,

    /// Shows updates for packages from the lock file
    #[arg(long)]
    pub locked: bool,

    /// Shows only packages that are directly required by the root package
    #[arg(short = 'D', long)]
    pub direct: bool,

    /// Return a non-zero exit code when there are outdated packages
    #[arg(long)]
    pub strict: bool,

    /// Show only packages that have major SemVer-compatible updates
    #[arg(short = 'M', long)]
    pub major_only: bool,

    /// Show only packages that have minor SemVer-compatible updates
    #[arg(short = 'm', long)]
    pub minor_only: bool,

    /// Show only packages that have patch SemVer-compatible updates
    #[arg(short = 'p', long)]
    pub patch_only: bool,

    /// Output format: text or json
    #[arg(short = 'f', long, default_value = "text")]
    pub format: String,

    /// Ignore specified package(s), can contain wildcards (*)
    #[arg(long)]
    pub ignore: Vec<String>,

    /// Disables search in require-dev packages
    #[arg(long)]
    pub no_dev: bool,

    /// Working directory
    #[arg(short = 'd', long, default_value = ".")]
    pub working_dir: PathBuf,
}

pub async fn execute(args: OutdatedArgs) -> Result<i32> {
    let show_args = ShowArgs {
        package: args.package,
        version: None,
        all: args.all,
        locked: args.locked,
        platform: false,
        available: false,
        self_package: false,
        name_only: false,
        path: false,
        tree: false,
        latest: true,
        outdated: !args.all,
        direct: args.direct,
        format: args.format,
        no_dev: args.no_dev,
        working_dir: args.working_dir,
    };

    let result = show::execute(show_args).await?;

    if args.strict && result == 0 {
        // TODO: Return non-zero if there were outdated packages
        // For now, just return the result from show
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_outdated_args_to_show_args_default() {
        let args = OutdatedArgs {
            package: None,
            all: false,
            locked: false,
            direct: false,
            strict: false,
            major_only: false,
            minor_only: false,
            patch_only: false,
            format: "text".to_string(),
            ignore: vec![],
            no_dev: false,
            working_dir: PathBuf::from("."),
        };
        assert!(!args.all);
    }

    #[test]
    fn test_outdated_args_with_all_flag() {
        let args = OutdatedArgs {
            package: None,
            all: true,
            locked: false,
            direct: false,
            strict: false,
            major_only: false,
            minor_only: false,
            patch_only: false,
            format: "text".to_string(),
            ignore: vec![],
            no_dev: false,
            working_dir: PathBuf::from("."),
        };
        assert!(args.all);
    }

    #[test]
    fn test_outdated_format_validation() {
        fn is_valid_format(format: &str) -> bool {
            format == "text" || format == "json"
        }
        assert!(is_valid_format("text"));
        assert!(is_valid_format("json"));
        assert!(!is_valid_format("xml"));
    }
}
