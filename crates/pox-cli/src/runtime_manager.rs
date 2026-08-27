use anyhow::{anyhow, bail, Context, Result};
use base64::Engine;
use ed25519_dalek::{Signature, VerifyingKey};
use pox_embed::{runtime_target, PhpRuntime};
use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::env;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use toml_edit::{value, DocumentMut};

const DEFAULT_INDEX_URL: &str =
    "https://github.com/shyim/pox-runtime/releases/download/runtime-index/index.json";
const DEFAULT_KEY_ID: &str = "pox-runtime-2026-01";
const TRUSTED_PUBLIC_KEYS: &[(&str, &str)] = &[(
    DEFAULT_KEY_ID,
    "aec394e1676614665e741df6a09a993a4b9501fbca8dfffc8e8374c1f69c47f3",
)];

#[derive(Debug, Clone, Deserialize)]
pub struct RuntimeManifest {
    pub schema: u32,
    pub abi_major: u16,
    pub abi_minor: u16,
    pub php_version: String,
    pub runtime_revision: String,
    pub target: String,
    pub zts: bool,
    pub library: String,
    pub library_sha256: String,
}

#[derive(Debug, Deserialize)]
struct ReleaseIndex {
    schema: u32,
    releases: Vec<RuntimeRelease>,
}

#[derive(Debug, Clone, Deserialize)]
struct RuntimeRelease {
    php_version: String,
    runtime_revision: String,
    abi_major: u16,
    abi_minor: u16,
    artifacts: Vec<RuntimeArtifact>,
}

#[derive(Debug, Clone, Deserialize)]
struct RuntimeArtifact {
    target: String,
    url: String,
    sha256: String,
}

#[derive(Debug, Clone)]
pub struct InstalledRuntime {
    pub manifest: RuntimeManifest,
    pub root: PathBuf,
}

impl InstalledRuntime {
    pub fn library_path(&self) -> PathBuf {
        self.root.join(&self.manifest.library)
    }
}

#[derive(Debug, Clone)]
pub struct RuntimeManager {
    data_dir: PathBuf,
    cache_dir: PathBuf,
    config_dir: PathBuf,
}

impl RuntimeManager {
    pub fn new() -> Result<Self> {
        let home = env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| anyhow!("HOME is not set"))?;
        let data_dir = env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local/share"))
            .join("pox");
        let cache_dir = env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".cache"))
            .join("pox");
        let config_dir = env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"))
            .join("pox");
        Ok(Self {
            data_dir,
            cache_dir,
            config_dir,
        })
    }

    fn runtimes_dir(&self) -> PathBuf {
        self.data_dir.join("runtimes")
    }

    fn global_config_path(&self) -> PathBuf {
        self.config_dir.join("config.toml")
    }

    pub fn install(&self, selector: &str, force: bool) -> Result<InstalledRuntime> {
        let index = self.fetch_index()?;
        let release = select_release(&index.releases, selector)
            .ok_or_else(|| anyhow!("no signed PHP runtime release matches {selector}"))?;
        if release.abi_major != 1 {
            bail!(
                "PHP {} runtime {} requires unsupported ABI {}.{}",
                release.php_version,
                release.runtime_revision,
                release.abi_major,
                release.abi_minor
            );
        }
        let artifact = release
            .artifacts
            .iter()
            .find(|artifact| artifact.target == runtime_target())
            .ok_or_else(|| {
                anyhow!(
                    "PHP {} runtime has no artifact for {}",
                    release.php_version,
                    runtime_target()
                )
            })?;

        let target = self
            .runtimes_dir()
            .join(&release.php_version)
            .join(&release.runtime_revision)
            .join(runtime_target());
        if target.exists() && !force {
            return self
                .read_installed(&target)
                .with_context(|| format!("runtime is already installed at {}", target.display()));
        }

        fs::create_dir_all(&self.cache_dir)?;
        let archive_path = self.cache_dir.join(format!(
            "pox-php-{}-{}-{}.tar.zst",
            release.php_version,
            release.runtime_revision,
            runtime_target()
        ));
        self.download(&artifact.url, &archive_path)?;
        verify_file_sha256(&archive_path, &artifact.sha256)?;

        let parent = target
            .parent()
            .ok_or_else(|| anyhow!("invalid runtime destination"))?;
        fs::create_dir_all(parent)?;
        let staging = tempfile::Builder::new()
            .prefix(".install-")
            .tempdir_in(parent)?;
        extract_archive(&archive_path, staging.path())?;
        let extracted_root = single_archive_root(staging.path())?;
        let installed = self.read_installed(&extracted_root)?;
        validate_manifest(&installed, &release.php_version, &release.runtime_revision)?;
        PhpRuntime::load(installed.library_path())
            .context("downloaded runtime failed ABI validation")?;

        if target.exists() {
            fs::remove_dir_all(&target)
                .with_context(|| format!("failed to replace runtime at {}", target.display()))?;
        }
        fs::rename(&extracted_root, &target)?;
        let installed = self.read_installed(&target)?;
        println!(
            "Installed PHP {} ({}) for {}",
            installed.manifest.php_version,
            installed.manifest.runtime_revision,
            installed.manifest.target
        );
        Ok(installed)
    }

    pub fn use_version(&self, selector: &str, global: bool) -> Result<InstalledRuntime> {
        let installed = self.find_installed(selector)?.ok_or_else(|| {
            anyhow!("PHP {selector} is not installed; run `pox php install {selector}`")
        })?;
        let path = if global {
            self.global_config_path()
        } else {
            project_config_path(&env::current_dir()?)
        };
        write_version(&path, &installed.manifest.php_version)?;
        println!(
            "Using PHP {}{}",
            installed.manifest.php_version,
            if global {
                " globally"
            } else {
                " in this project"
            }
        );
        Ok(installed)
    }

    pub fn list(&self, remote: bool) -> Result<()> {
        if remote {
            let index = self.fetch_index()?;
            let mut releases = index.releases;
            releases.sort_by(|left, right| compare_releases(right, left));
            releases.dedup_by(|a, b| a.php_version == b.php_version);
            for release in releases {
                let available = release
                    .artifacts
                    .iter()
                    .any(|artifact| artifact.target == runtime_target());
                if available {
                    println!("{}\t{}", release.php_version, release.runtime_revision);
                }
            }
            return Ok(());
        }

        let current = self.selected_version(&env::current_dir()?)?;
        for runtime in self.installed()? {
            let marker = if current
                .as_deref()
                .is_some_and(|selector| version_matches(&runtime.manifest.php_version, selector))
            {
                "*"
            } else {
                " "
            };
            println!(
                "{} {}\t{}\t{}",
                marker,
                runtime.manifest.php_version,
                runtime.manifest.runtime_revision,
                runtime.manifest.target
            );
        }
        Ok(())
    }

    pub fn current(&self) -> Result<InstalledRuntime> {
        let selector = self
            .selected_version(&env::current_dir()?)?
            .ok_or_else(|| {
                anyhow!("no PHP runtime selected; run `pox php use <version> --global`")
            })?;
        self.find_installed(&selector)?.ok_or_else(|| {
            anyhow!(
                "PHP {selector} is selected but not installed; run `pox php install {selector}`"
            )
        })
    }

    pub fn remove(&self, selector: &str, force: bool) -> Result<()> {
        let installed = self
            .find_installed(selector)?
            .ok_or_else(|| anyhow!("PHP {selector} is not installed"))?;
        let selected = self.selected_version(&env::current_dir()?)?;
        if selected.as_deref() == Some(&installed.manifest.php_version) && !force {
            bail!(
                "PHP {} is active; select another runtime or pass --force",
                installed.manifest.php_version
            );
        }
        let matching = self
            .installed()?
            .into_iter()
            .filter(|runtime| runtime.manifest.php_version == installed.manifest.php_version)
            .collect::<Vec<_>>();
        for runtime in matching {
            fs::remove_dir_all(&runtime.root)
                .with_context(|| format!("failed to remove {}", runtime.root.display()))?;
        }
        println!(
            "Removed PHP {} ({})",
            installed.manifest.php_version, installed.manifest.runtime_revision
        );
        Ok(())
    }

    pub fn load_selected(&self) -> Result<PhpRuntime> {
        if let Some(path) = env::var_os("POX_PHP_RUNTIME") {
            return PhpRuntime::load(path).context("failed to load POX_PHP_RUNTIME");
        }
        let installed = self.current()?;
        PhpRuntime::load(installed.library_path()).with_context(|| {
            format!(
                "failed to load PHP {} runtime from {}",
                installed.manifest.php_version,
                installed.library_path().display()
            )
        })
    }

    fn selected_version(&self, cwd: &Path) -> Result<Option<String>> {
        if let Ok(version) = env::var("POX_PHP_VERSION") {
            if !version.trim().is_empty() {
                return Ok(Some(version));
            }
        }
        if let Some(path) = find_project_config(cwd) {
            if let Some(version) = read_version(&path)? {
                return Ok(Some(version));
            }
        }
        read_version(&self.global_config_path())
    }

    fn installed(&self) -> Result<Vec<InstalledRuntime>> {
        let root = self.runtimes_dir();
        if !root.exists() {
            return Ok(Vec::new());
        }
        let mut runtimes = Vec::new();
        for version in fs::read_dir(root)? {
            let version = version?;
            if !version.file_type()?.is_dir() {
                continue;
            }
            for revision in fs::read_dir(version.path())? {
                let revision = revision?;
                if !revision.file_type()?.is_dir() {
                    continue;
                }
                let target = revision.path().join(runtime_target());
                if target.is_dir() {
                    if let Ok(runtime) = self.read_installed(&target) {
                        runtimes.push(runtime);
                    }
                }
            }
        }
        runtimes.sort_by(compare_installed);
        Ok(runtimes)
    }

    fn find_installed(&self, selector: &str) -> Result<Option<InstalledRuntime>> {
        Ok(self
            .installed()?
            .into_iter()
            .filter(|runtime| version_matches(&runtime.manifest.php_version, selector))
            .max_by(compare_installed))
    }

    fn read_installed(&self, root: &Path) -> Result<InstalledRuntime> {
        let manifest_path = root.join("runtime.json");
        let manifest: RuntimeManifest = serde_json::from_reader(
            File::open(&manifest_path)
                .with_context(|| format!("failed to open {}", manifest_path.display()))?,
        )?;
        Ok(InstalledRuntime {
            manifest,
            root: root.to_path_buf(),
        })
    }

    fn fetch_index(&self) -> Result<ReleaseIndex> {
        let index_url =
            env::var("POX_RUNTIME_INDEX_URL").unwrap_or_else(|_| DEFAULT_INDEX_URL.to_string());
        let signature_url = env::var("POX_RUNTIME_INDEX_SIGNATURE_URL")
            .unwrap_or_else(|_| format!("{index_url}.sig"));
        let index = fetch_bytes(&index_url)?;
        let signature = fetch_bytes(&signature_url)?;
        verify_index_signature(&index, &signature)?;
        let parsed: ReleaseIndex = serde_json::from_slice(&index)?;
        if parsed.schema != 1 {
            bail!("unsupported runtime index schema {}", parsed.schema);
        }
        Ok(parsed)
    }

    fn download(&self, url: &str, output: &Path) -> Result<()> {
        if let Some(path) = url.strip_prefix("file://") {
            fs::copy(path, output)?;
            return Ok(());
        }
        let mut response = http_client()?
            .get(url)
            .send()?
            .error_for_status()
            .with_context(|| format!("failed to download {url}"))?;
        let mut file = File::create(output)?;
        response.copy_to(&mut file)?;
        file.flush()?;
        Ok(())
    }
}

fn http_client() -> Result<reqwest::blocking::Client> {
    Ok(reqwest::blocking::Client::builder()
        .user_agent(format!("pox/{}", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(300))
        .build()?)
}

fn fetch_bytes(url: &str) -> Result<Vec<u8>> {
    if let Some(path) = url.strip_prefix("file://") {
        return Ok(fs::read(path)?);
    }
    Ok(http_client()?
        .get(url)
        .send()?
        .error_for_status()
        .with_context(|| format!("failed to download {url}"))?
        .bytes()?
        .to_vec())
}

fn verify_index_signature(index: &[u8], encoded_signature: &[u8]) -> Result<()> {
    let encoded = std::str::from_utf8(encoded_signature)?.trim();
    let (key_id, encoded) = encoded.split_once(':').unwrap_or((DEFAULT_KEY_ID, encoded));
    let key_hex = if let Ok(override_key) = env::var("POX_RUNTIME_PUBLIC_KEY") {
        override_key
    } else {
        TRUSTED_PUBLIC_KEYS
            .iter()
            .find_map(|(trusted_id, key)| (*trusted_id == key_id).then_some(*key))
            .ok_or_else(|| anyhow!("runtime index uses untrusted signing key {key_id}"))?
            .to_string()
    };
    verify_index_signature_with_key(index, encoded.as_bytes(), &key_hex)
}

fn verify_index_signature_with_key(
    index: &[u8],
    encoded_signature: &[u8],
    key_hex: &str,
) -> Result<()> {
    let key_bytes: [u8; 32] = hex::decode(key_hex.trim())?
        .try_into()
        .map_err(|_| anyhow!("runtime public key must contain 32 bytes"))?;
    let signature_bytes = base64::engine::general_purpose::STANDARD.decode(
        encoded_signature
            .iter()
            .copied()
            .filter(|byte| !byte.is_ascii_whitespace())
            .collect::<Vec<_>>(),
    )?;
    let signature = Signature::from_slice(&signature_bytes)?;
    VerifyingKey::from_bytes(&key_bytes)?
        .verify_strict(index, &signature)
        .context("runtime index signature verification failed")
}

fn select_release<'a>(
    releases: &'a [RuntimeRelease],
    selector: &str,
) -> Option<&'a RuntimeRelease> {
    releases
        .iter()
        .filter(|release| release.abi_major == 1)
        .filter(|release| version_matches(&release.php_version, selector))
        .max_by(|left, right| compare_releases(left, right))
}

fn compare_releases(left: &RuntimeRelease, right: &RuntimeRelease) -> Ordering {
    parsed_version(&left.php_version)
        .cmp(&parsed_version(&right.php_version))
        .then_with(|| {
            revision_number(&left.runtime_revision).cmp(&revision_number(&right.runtime_revision))
        })
}

fn compare_installed(left: &InstalledRuntime, right: &InstalledRuntime) -> Ordering {
    parsed_version(&left.manifest.php_version)
        .cmp(&parsed_version(&right.manifest.php_version))
        .then_with(|| {
            revision_number(&left.manifest.runtime_revision)
                .cmp(&revision_number(&right.manifest.runtime_revision))
        })
}

fn parsed_version(value: &str) -> Version {
    Version::parse(value).unwrap_or_else(|_| Version::new(0, 0, 0))
}

fn revision_number(value: &str) -> u64 {
    value.trim_start_matches('r').parse().unwrap_or(0)
}

fn version_matches(version: &str, selector: &str) -> bool {
    let Ok(version) = Version::parse(version) else {
        return false;
    };
    let parts = selector.split('.').collect::<Vec<_>>();
    let Ok(major) = parts.first().unwrap_or(&"").parse::<u64>() else {
        return false;
    };
    if version.major != major {
        return false;
    }
    if let Some(minor) = parts.get(1) {
        if minor.parse::<u64>().ok() != Some(version.minor) {
            return false;
        }
    }
    if let Some(patch) = parts.get(2) {
        if patch.parse::<u64>().ok() != Some(version.patch) {
            return false;
        }
    }
    parts.len() <= 3
}

fn verify_file_sha256(path: &Path, expected: &str) -> Result<()> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let actual = hex::encode(hasher.finalize());
    if !actual.eq_ignore_ascii_case(expected) {
        bail!("runtime archive checksum mismatch: expected {expected}, got {actual}");
    }
    Ok(())
}

fn extract_archive(path: &Path, destination: &Path) -> Result<()> {
    let file = File::open(path)?;
    let decoder = zstd::Decoder::new(file)?;
    let mut archive = tar::Archive::new(decoder);
    for entry in archive.entries()? {
        let mut entry = entry?;
        let entry_path = entry.path()?.into_owned();
        if entry_path.is_absolute()
            || entry_path.components().any(|component| {
                matches!(
                    component,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            })
        {
            bail!(
                "runtime archive contains unsafe path {}",
                entry_path.display()
            );
        }
        entry.unpack_in(destination)?;
    }
    Ok(())
}

fn single_archive_root(staging: &Path) -> Result<PathBuf> {
    let entries = fs::read_dir(staging)?.collect::<std::io::Result<Vec<_>>>()?;
    if entries.len() != 1 || !entries[0].file_type()?.is_dir() {
        bail!("runtime archive must contain exactly one root directory");
    }
    Ok(entries[0].path())
}

fn validate_manifest(installed: &InstalledRuntime, version: &str, revision: &str) -> Result<()> {
    let manifest = &installed.manifest;
    if manifest.schema != 1
        || manifest.php_version != version
        || manifest.runtime_revision != revision
        || manifest.target != runtime_target()
        || manifest.abi_major != 1
        || !manifest.zts
    {
        bail!("downloaded runtime manifest does not match the selected artifact");
    }
    let library = installed.library_path();
    if !library.is_file() {
        bail!("runtime library {} is missing", library.display());
    }
    verify_file_sha256(&library, &manifest.library_sha256)
}

fn find_project_config(start: &Path) -> Option<PathBuf> {
    let mut current = start.to_path_buf();
    loop {
        let candidate = current.join("pox.toml");
        if candidate.is_file() {
            return Some(candidate);
        }
        if !current.pop() {
            return None;
        }
    }
}

fn project_config_path(cwd: &Path) -> PathBuf {
    find_project_config(cwd).unwrap_or_else(|| cwd.join("pox.toml"))
}

fn read_version(path: &Path) -> Result<Option<String>> {
    if !path.is_file() {
        return Ok(None);
    }
    let document = fs::read_to_string(path)?.parse::<DocumentMut>()?;
    Ok(document["php"]["version"].as_str().map(ToString::to_string))
}

fn write_version(path: &Path, version: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut document = if path.is_file() {
        fs::read_to_string(path)?.parse::<DocumentMut>()?
    } else {
        DocumentMut::new()
    };
    document["php"]["version"] = value(version);
    let temporary = path.with_extension("toml.tmp");
    fs::write(&temporary, document.to_string())?;
    fs::rename(temporary, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_selectors_are_component_based() {
        assert!(version_matches("8.5.9", "8"));
        assert!(version_matches("8.5.9", "8.5"));
        assert!(version_matches("8.5.9", "8.5.9"));
        assert!(!version_matches("8.5.9", "8.4"));
        assert!(!version_matches("8.5.9", "8.5.8"));
        assert!(!version_matches("8.5.9", "latest"));
    }

    #[test]
    fn writes_version_without_losing_ini_configuration() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("pox.toml");
        fs::write(&path, "[php.ini]\nmemory_limit = \"256M\"\n").unwrap();
        write_version(&path, "8.5.9").unwrap();
        let content = fs::read_to_string(path).unwrap();
        assert!(content.contains("version = \"8.5.9\""));
        assert!(content.contains("memory_limit = \"256M\""));
    }

    #[test]
    fn verifies_the_production_signing_key_and_rejects_tampering() {
        let signature = b"DRUqfBHSFmocy36p86bkZDu+8sREQ/hG9ACWqhmEfUIREBms4i04r7dZwsJP4ZQyNXNYvIfGm7BsVBbmP4yKAA==";
        verify_index_signature_with_key(b"signed-index-test", signature, TRUSTED_PUBLIC_KEYS[0].1)
            .unwrap();
        assert!(verify_index_signature_with_key(
            b"tampered-index-test",
            signature,
            TRUSTED_PUBLIC_KEYS[0].1
        )
        .is_err());
    }
}
