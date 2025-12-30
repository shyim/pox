//! Git repository downloader.

use git2::{
    build::RepoBuilder, Cred, FetchOptions, RemoteCallbacks, Repository,
};
use std::path::Path;

use crate::{ComposerError, Result};

/// Git repository downloader
pub struct GitDownloader {
    /// SSH key path for authentication (optional)
    ssh_key: Option<std::path::PathBuf>,
    /// Whether to use the system SSH agent
    use_ssh_agent: bool,
}

impl GitDownloader {
    /// Create a new Git downloader
    pub fn new() -> Self {
        Self {
            ssh_key: None,
            use_ssh_agent: true,
        }
    }

    /// Set SSH key for authentication
    pub fn with_ssh_key(mut self, path: impl Into<std::path::PathBuf>) -> Self {
        self.ssh_key = Some(path.into());
        self
    }

    /// Disable SSH agent
    pub fn without_ssh_agent(mut self) -> Self {
        self.use_ssh_agent = false;
        self
    }

    /// Clone a repository
    pub fn clone(&self, url: &str, dest: &Path, reference: Option<&str>) -> Result<()> {
        let mut callbacks = RemoteCallbacks::new();

        // Set up credentials callback
        let ssh_key = self.ssh_key.clone();
        let use_ssh_agent = self.use_ssh_agent;

        callbacks.credentials(move |_url, username_from_url, allowed_types| {
            // Try SSH key authentication
            if allowed_types.contains(git2::CredentialType::SSH_KEY) {
                let username = username_from_url.unwrap_or("git");

                // Try explicit SSH key first
                if let Some(ref key_path) = ssh_key {
                    return Cred::ssh_key(username, None, key_path, None);
                }

                // Try SSH agent
                if use_ssh_agent {
                    return Cred::ssh_key_from_agent(username);
                }

                // Try default SSH key locations
                if let Some(home) = dirs::home_dir() {
                    let id_rsa = home.join(".ssh/id_rsa");
                    let id_ed25519 = home.join(".ssh/id_ed25519");

                    if id_ed25519.exists() {
                        return Cred::ssh_key(username, None, &id_ed25519, None);
                    }
                    if id_rsa.exists() {
                        return Cred::ssh_key(username, None, &id_rsa, None);
                    }
                }
            }

            // Try username/password for HTTPS
            if allowed_types.contains(git2::CredentialType::USER_PASS_PLAINTEXT) {
                // Check for environment variables
                if let (Ok(user), Ok(pass)) = (
                    std::env::var("COMPOSER_AUTH_USER"),
                    std::env::var("COMPOSER_AUTH_PASS"),
                ) {
                    return Cred::userpass_plaintext(&user, &pass);
                }
            }

            // Default - no credentials
            if allowed_types.contains(git2::CredentialType::DEFAULT) {
                return Cred::default();
            }

            Err(git2::Error::from_str("no valid credentials found"))
        });

        let mut fetch_opts = FetchOptions::new();
        fetch_opts.remote_callbacks(callbacks);

        // Clone the repository
        let mut builder = RepoBuilder::new();
        builder.fetch_options(fetch_opts);

        // Clone into destination
        let repo = builder
            .clone(url, dest)
            .map_err(|e| ComposerError::Git(e))?;

        // Checkout specific reference if provided
        if let Some(ref_name) = reference {
            self.checkout(&repo, ref_name)?;
        }

        Ok(())
    }

    /// Checkout a specific reference (branch, tag, or commit)
    fn checkout(&self, repo: &Repository, reference: &str) -> Result<()> {
        // Try as a commit hash first
        if reference.len() >= 7 {
            if let Ok(oid) = git2::Oid::from_str(reference) {
                let commit = repo.find_commit(oid)?;
                repo.checkout_tree(commit.as_object(), None)?;
                repo.set_head_detached(oid)?;
                return Ok(());
            }
        }

        // Try as a tag
        let tag_ref = format!("refs/tags/{}", reference);
        if let Ok(reference_obj) = repo.find_reference(&tag_ref) {
            let commit = reference_obj.peel_to_commit()?;
            repo.checkout_tree(commit.as_object(), None)?;
            repo.set_head_detached(commit.id())?;
            return Ok(());
        }

        // Try as a branch
        let branch_ref = format!("refs/remotes/origin/{}", reference);
        if let Ok(reference_obj) = repo.find_reference(&branch_ref) {
            let commit = reference_obj.peel_to_commit()?;
            repo.checkout_tree(commit.as_object(), None)?;
            repo.set_head_detached(commit.id())?;
            return Ok(());
        }

        // Try revparse as fallback
        let obj = repo.revparse_single(reference)?;
        repo.checkout_tree(&obj, None)?;
        if let Some(commit) = obj.as_commit() {
            repo.set_head_detached(commit.id())?;
        }

        Ok(())
    }

    /// Update an existing repository
    pub fn update(&self, repo_path: &Path, reference: Option<&str>) -> Result<()> {
        let repo = Repository::open(repo_path)?;

        // Fetch updates
        let mut remote = repo.find_remote("origin")?;

        let mut callbacks = RemoteCallbacks::new();
        let ssh_key = self.ssh_key.clone();
        let use_ssh_agent = self.use_ssh_agent;

        callbacks.credentials(move |_url, username_from_url, allowed_types| {
            if allowed_types.contains(git2::CredentialType::SSH_KEY) {
                let username = username_from_url.unwrap_or("git");

                if let Some(ref key_path) = ssh_key {
                    return Cred::ssh_key(username, None, key_path, None);
                }

                if use_ssh_agent {
                    return Cred::ssh_key_from_agent(username);
                }
            }

            Cred::default()
        });

        let mut fetch_opts = FetchOptions::new();
        fetch_opts.remote_callbacks(callbacks);

        remote.fetch(&["refs/heads/*:refs/remotes/origin/*"], Some(&mut fetch_opts), None)?;

        // Checkout specific reference if provided
        if let Some(ref_name) = reference {
            self.checkout(&repo, ref_name)?;
        }

        Ok(())
    }

    /// Get the current commit hash
    pub fn get_head_commit(repo_path: &Path) -> Result<String> {
        let repo = Repository::open(repo_path)?;
        let head = repo.head()?;
        let commit = head.peel_to_commit()?;
        Ok(commit.id().to_string())
    }

    /// Check if a path is a git repository
    pub fn is_git_repo(path: &Path) -> bool {
        Repository::open(path).is_ok()
    }
}

impl Default for GitDownloader {
    fn default() -> Self {
        Self::new()
    }
}

/// Helper module for getting home directory
mod dirs {
    use std::path::PathBuf;

    pub fn home_dir() -> Option<PathBuf> {
        std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_git_downloader_creation() {
        let downloader = GitDownloader::new();
        assert!(downloader.ssh_key.is_none());
        assert!(downloader.use_ssh_agent);
    }

    #[test]
    fn test_git_downloader_with_ssh_key() {
        let downloader = GitDownloader::new()
            .with_ssh_key("/path/to/key");
        assert_eq!(downloader.ssh_key, Some(std::path::PathBuf::from("/path/to/key")));
    }

    #[test]
    fn test_is_not_git_repo() {
        let temp_dir = TempDir::new().unwrap();
        assert!(!GitDownloader::is_git_repo(temp_dir.path()));
    }

    #[test]
    fn test_is_git_repo() {
        let temp_dir = TempDir::new().unwrap();
        Repository::init(temp_dir.path()).unwrap();
        assert!(GitDownloader::is_git_repo(temp_dir.path()));
    }

    #[test]
    #[ignore] // Requires network access
    fn test_clone_public_repo() {
        let temp_dir = TempDir::new().unwrap();
        let downloader = GitDownloader::new();

        let result = downloader.clone(
            "https://github.com/octocat/Hello-World.git",
            temp_dir.path(),
            None,
        );

        assert!(result.is_ok());
        assert!(temp_dir.path().join(".git").exists());
    }
}
