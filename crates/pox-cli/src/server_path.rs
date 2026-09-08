//! Request-target decoding and document-root confinement shared by both server modes.
use std::path::{Path, PathBuf};

pub fn parse_target(target: &str) -> Result<(String, String), u16> {
    if !target.starts_with('/') || target.contains('#') {
        return Err(400);
    }
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let mut decoded = Vec::with_capacity(path.len());
    let mut bytes = path.bytes();
    while let Some(byte) = bytes.next() {
        let byte = if byte == b'%' {
            let high = bytes.next().and_then(|b| (b as char).to_digit(16));
            let low = bytes.next().and_then(|b| (b as char).to_digit(16));
            match (high, low) {
                (Some(high), Some(low)) => (high * 16 + low) as u8,
                _ => return Err(400),
            }
        } else {
            byte
        };
        if byte < 32 || byte == 127 || byte == b'\\' {
            return Err(400);
        }
        decoded.push(byte);
    }
    let path = String::from_utf8(decoded).map_err(|_| 400u16)?;
    for component in path.split('/') {
        if component == "." || component == ".." {
            return Err(403);
        }
        // Permit ACME and other standard well-known resources, but not nested dotfiles.
        if component.starts_with('.') && component != ".well-known" {
            return Err(403);
        }
    }
    Ok((path, query.to_owned()))
}

pub fn confined_path(root: &Path, path: &str) -> Result<PathBuf, u16> {
    let candidate = root.join(path.trim_start_matches('/'));
    // Check existing ancestors too: a missing file below an escaping symlink must
    // not fall through to the front controller.
    for ancestor in candidate.ancestors() {
        match ancestor.canonicalize() {
            Ok(resolved) => {
                if !resolved.starts_with(root) {
                    return Err(403);
                }
                return Ok(candidate);
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                ) =>
            {
                continue
            }
            Err(_) => return Err(403),
        }
    }
    Err(403)
}

pub fn is_php(path: &Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("php"))
}

pub fn script(root: &Path, path: &str, router: Option<&Path>) -> Result<PathBuf, u16> {
    if let Some(router) = router {
        return Ok(router.to_path_buf());
    }
    // Only an existing PHP file can terminate the script portion of PATH_INFO.
    // A directory named *.php must not mask a later script component.
    for (index, _) in path.match_indices('/').skip(1) {
        let prefix = &path[..index];
        if is_php(Path::new(prefix)) {
            let candidate = confined_path(root, prefix)?;
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    let mut candidate = confined_path(root, path)?;
    if candidate.is_dir() {
        candidate = confined_path(root, &format!("{}/index.php", path.trim_end_matches('/')))?;
    }
    if candidate.is_file() {
        return if is_php(&candidate) {
            Ok(candidate)
        } else {
            Err(404)
        };
    }
    let index = confined_path(root, "/index.php")?;
    if index.is_file() {
        Ok(index)
    } else {
        Err(404)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_once_and_preserve_query() {
        assert_eq!(
            parse_target("/hello%20world.php?a=%2f+b").unwrap(),
            ("/hello world.php".into(), "a=%2f+b".into())
        );
        assert_eq!(parse_target("/%252e%252e/a").unwrap().0, "/%2e%2e/a");
        assert!(parse_target("/.well-known/acme-challenge/token").is_ok());
    }

    #[test]
    fn reject_ambiguous_and_private_paths() {
        for path in [
            "/../secret",
            "/%2e%2e/secret",
            "/a%2f..%2fsecret",
            "/.env",
            "/.git/config",
            "/.well-known/.env",
            "/./index.php",
        ] {
            assert_eq!(parse_target(path), Err(403), "{path}");
        }
        for path in [
            "/a%",
            "/%gg",
            "/%00",
            "/%ff",
            "/%5csecret",
            "http://host/a",
            "*",
            "/a#b",
            "/a\n",
        ] {
            assert_eq!(parse_target(path), Err(400), "{path}");
        }
    }

    #[test]
    fn only_execute_php_and_confine_indexes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::write(root.join("index.php"), "<?php").unwrap();
        std::fs::write(root.join("asset.txt"), "data").unwrap();
        std::fs::write(root.join("upper.PHP"), "<?php").unwrap();
        assert_eq!(
            script(&root, "/route", None).unwrap(),
            root.join("index.php")
        );
        assert_eq!(script(&root, "/asset.txt", None), Err(404));
        assert_eq!(
            script(&root, "/upper.PHP", None).unwrap(),
            root.join("upper.PHP")
        );
    }

    #[cfg(unix)]
    #[test]
    fn reject_symlink_escape_including_missing_children_and_index() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::os::unix::fs::symlink(outside.path(), root.join("escape")).unwrap();
        std::fs::write(outside.path().join("secret"), "private").unwrap();
        assert_eq!(confined_path(&root, "/escape/secret"), Err(403));
        assert_eq!(confined_path(&root, "/escape/missing"), Err(403));
        std::os::unix::fs::symlink(outside.path().join("secret"), root.join("index.php")).unwrap();
        assert_eq!(script(&root, "/", None), Err(403));
        assert_eq!(script(&root, "/missing", None), Err(403));
    }
}
