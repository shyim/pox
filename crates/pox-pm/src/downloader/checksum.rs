//! Checksum verification for downloaded files.

use sha2::{Sha256, Sha384, Sha512, Digest};
use md5::Md5;
use std::path::Path;
use tokio::io::AsyncReadExt;

use crate::Result;

/// Supported checksum types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChecksumType {
    Sha1,
    Sha256,
    Sha384,
    Sha512,
    Md5,
}

impl ChecksumType {
    /// Detect checksum type from length of hex string
    pub fn from_hex_length(len: usize) -> Option<Self> {
        match len {
            32 => Some(ChecksumType::Md5),
            40 => Some(ChecksumType::Sha1),
            64 => Some(ChecksumType::Sha256),
            96 => Some(ChecksumType::Sha384),
            128 => Some(ChecksumType::Sha512),
            _ => None,
        }
    }
}

/// Verify checksum of a file
pub async fn verify_checksum(
    path: &Path,
    expected: &str,
    checksum_type: ChecksumType,
) -> Result<bool> {
    let mut file = tokio::fs::File::open(path).await?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer).await?;

    let actual = match checksum_type {
        ChecksumType::Sha1 => {
            use sha1::{Sha1, Digest as Sha1Digest};
            let mut hasher = Sha1::new();
            hasher.update(&buffer);
            format!("{:x}", hasher.finalize())
        }
        ChecksumType::Sha256 => {
            let mut hasher = Sha256::new();
            hasher.update(&buffer);
            format!("{:x}", hasher.finalize())
        }
        ChecksumType::Sha384 => {
            let mut hasher = Sha384::new();
            hasher.update(&buffer);
            format!("{:x}", hasher.finalize())
        }
        ChecksumType::Sha512 => {
            let mut hasher = Sha512::new();
            hasher.update(&buffer);
            format!("{:x}", hasher.finalize())
        }
        ChecksumType::Md5 => {
            let mut hasher = Md5::new();
            hasher.update(&buffer);
            format!("{:x}", hasher.finalize())
        }
    };

    Ok(actual.eq_ignore_ascii_case(expected))
}

/// Compute SHA-256 checksum of a file
#[allow(dead_code)]
pub async fn compute_sha256(path: &Path) -> Result<String> {
    let mut file = tokio::fs::File::open(path).await?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer).await?;

    let mut hasher = Sha256::new();
    hasher.update(&buffer);
    Ok(format!("{:x}", hasher.finalize()))
}

/// Compute SHA-1 checksum of a file
#[allow(dead_code)]
pub async fn compute_sha1(path: &Path) -> Result<String> {
    use sha1::{Sha1, Digest as Sha1Digest};

    let mut file = tokio::fs::File::open(path).await?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer).await?;

    let mut hasher = Sha1::new();
    hasher.update(&buffer);
    Ok(format!("{:x}", hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;
    use tokio::io::AsyncWriteExt;

    #[tokio::test]
    async fn test_checksum_type_from_hex_length() {
        assert_eq!(ChecksumType::from_hex_length(32), Some(ChecksumType::Md5));
        assert_eq!(ChecksumType::from_hex_length(40), Some(ChecksumType::Sha1));
        assert_eq!(ChecksumType::from_hex_length(64), Some(ChecksumType::Sha256));
        assert_eq!(ChecksumType::from_hex_length(96), Some(ChecksumType::Sha384));
        assert_eq!(ChecksumType::from_hex_length(128), Some(ChecksumType::Sha512));
        assert_eq!(ChecksumType::from_hex_length(50), None);
    }

    #[tokio::test]
    async fn test_verify_sha256() {
        let temp_file = NamedTempFile::new().unwrap();
        let path = temp_file.path();

        // Write test content
        let mut file = tokio::fs::File::create(path).await.unwrap();
        file.write_all(b"hello world").await.unwrap();
        file.flush().await.unwrap();
        drop(file);

        // SHA-256 of "hello world"
        let expected = "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9";

        let result = verify_checksum(path, expected, ChecksumType::Sha256).await;
        assert!(result.is_ok());
        assert!(result.unwrap());
    }

    #[tokio::test]
    async fn test_verify_sha256_mismatch() {
        let temp_file = NamedTempFile::new().unwrap();
        let path = temp_file.path();

        let mut file = tokio::fs::File::create(path).await.unwrap();
        file.write_all(b"hello world").await.unwrap();
        file.flush().await.unwrap();
        drop(file);

        let wrong_hash = "0000000000000000000000000000000000000000000000000000000000000000";

        let result = verify_checksum(path, wrong_hash, ChecksumType::Sha256).await;
        assert!(result.is_ok());
        assert!(!result.unwrap());
    }

    #[tokio::test]
    async fn test_compute_sha256() {
        let temp_file = NamedTempFile::new().unwrap();
        let path = temp_file.path();

        let mut file = tokio::fs::File::create(path).await.unwrap();
        file.write_all(b"hello world").await.unwrap();
        file.flush().await.unwrap();
        drop(file);

        let hash = compute_sha256(path).await.unwrap();
        assert_eq!(hash, "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9");
    }
}
