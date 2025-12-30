//! Package downloading and extraction module.
//!
//! This module provides functionality for downloading packages from
//! various sources (HTTP archives, Git repositories, local paths) and extracting them.

mod archive;
mod file;
mod git;
mod manager;
mod checksum;
mod path;

pub use archive::{ArchiveExtractor, ArchiveType};
pub use file::FileDownloader;
pub use git::GitDownloader;
pub use manager::{DownloadManager, DownloadResult, DownloadConfig};
pub use checksum::{verify_checksum, ChecksumType};
pub use path::{PathDownloader, PathStrategy, PathInstallResult};
