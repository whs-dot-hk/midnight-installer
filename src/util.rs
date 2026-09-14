use anyhow::Context;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use nix::unistd::{chown, Gid, Group, Uid, User};
use url::Url;
use walkdir::WalkDir;

/// Remove a file, treating "it was not there" as success: a revert should not fail because
/// the thing it undoes is already gone
#[tracing::instrument(skip(path), fields(path = %path.display()))]
pub(crate) async fn remove_file_if_exists(path: &Path) -> std::io::Result<()> {
    tracing::trace!("Removing file");
    match tokio::fs::remove_file(path).await {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            tracing::trace!("Ignoring nonexistent file");
            Ok(())
        },
        e @ Err(_) => e,
    }
}

/// Remove a directory and its contents, treating a missing directory as success
#[tracing::instrument(skip(path), fields(path = %path.display()))]
pub(crate) async fn remove_dir_all_if_exists(path: &Path) -> std::io::Result<()> {
    tracing::trace!("Removing directory and all contents");
    match tokio::fs::remove_dir_all(path).await {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            tracing::trace!("Ignoring nonexistent directory");
            Ok(())
        },
        e @ Err(_) => e,
    }
}

/// Whether `path` is a file with something in it
pub(crate) async fn file_has_contents(path: &Path) -> bool {
    match tokio::fs::metadata(path).await {
        Ok(metadata) => metadata.is_file() && metadata.len() > 0,
        Err(_) => false,
    }
}

/// Whether `path` is a directory with at least one entry
pub(crate) async fn directory_has_contents(path: &Path) -> bool {
    match tokio::fs::read_dir(path).await {
        Ok(mut entries) => matches!(entries.next_entry().await, Ok(Some(_))),
        Err(_) => false,
    }
}

/// Write a file by way of a temporary sibling which gets its mode and owner first, then is
/// renamed into place
///
/// A file which may hold a secret is therefore never briefly readable by anyone else under
/// its real name, and a failed write never leaves a truncated file behind.
pub(crate) async fn write_atomic(
    path: &Path,
    contents: &str,
    mode: u32,
    owner: Option<(Uid, Option<Gid>)>,
) -> std::io::Result<()> {
    use tokio::io::AsyncWriteExt;

    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let temp = path.with_extension("tmp");
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .mode(mode)
        .open(&temp)
        .await?;
    file.write_all(contents.as_bytes()).await?;
    file.flush().await?;
    drop(file);

    // The mode above is masked by the umask, so set it outright
    tokio::fs::set_permissions(&temp, PermissionsExt::from_mode(mode)).await?;
    if let Some((uid, gid)) = owner {
        chown(temp.as_path(), Some(uid), gid)?;
    }

    tokio::fs::rename(&temp, path).await
}

/// [`write_atomic`] with the owner given by name
pub(crate) async fn write_owned_file(
    path: &Path,
    contents: &str,
    mode: u32,
    user: Option<&str>,
) -> anyhow::Result<()> {
    let owner = match user {
        Some(user) => Some((uid_of(user)?, gid_of(user).ok())),
        None => None,
    };
    write_atomic(path, contents, mode, owner)
        .await
        .with_context(|| format!("Writing `{}`", path.display()))
}

pub(crate) fn uid_of(user: &str) -> anyhow::Result<Uid> {
    Ok(User::from_name(user)
        .with_context(|| format!("Getting user `{user}`"))?
        .with_context(|| format!("No user `{user}` on this system, create it first"))?
        .uid)
}

pub(crate) fn gid_of(group: &str) -> anyhow::Result<Gid> {
    Ok(Group::from_name(group)
        .with_context(|| format!("Getting group `{group}`"))?
        .with_context(|| format!("No group `{group}` on this system, create it first"))?
        .gid)
}

pub(crate) fn resolve_owner(
    user: Option<&str>,
    group: Option<&str>,
) -> anyhow::Result<(Option<Uid>, Option<Gid>)> {
    let uid = match user {
        Some(user) => Some(uid_of(user)?),
        None => None,
    };
    let gid = match group {
        Some(group) => Some(gid_of(group)?),
        None => None,
    };
    Ok((uid, gid))
}

/// `chown -R`
pub(crate) fn chown_recursive(
    path: &Path,
    uid: Option<Uid>,
    gid: Option<Gid>,
) -> anyhow::Result<()> {
    if uid.is_none() && gid.is_none() {
        return Ok(());
    }
    for entry in WalkDir::new(path).follow_links(false) {
        let entry = entry.with_context(|| format!("Walking directory `{}`", path.display()))?;
        chown(entry.path(), uid, gid)
            .with_context(|| format!("Changing the owner of `{}`", entry.path().display()))?;
    }
    Ok(())
}

/// Find the first file named `name` below `root`, deterministically
pub(crate) fn find_file(root: &Path, name: &str) -> Option<PathBuf> {
    WalkDir::new(root)
        .sort_by_file_name()
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .find(|entry| entry.file_name() == name)
        .map(|entry| entry.path().to_path_buf())
}

/// Find the first directory below `root` whose path ends with `suffix`
pub(crate) fn find_dir(root: &Path, suffix: &str) -> Option<PathBuf> {
    WalkDir::new(root)
        .sort_by_file_name()
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_dir())
        .find(|entry| entry.path().ends_with(suffix))
        .map(|entry| entry.path().to_path_buf())
}

/// Download `url` into memory
pub(crate) async fn fetch_bytes(url: &Url) -> anyhow::Result<Vec<u8>> {
    tracing::debug!(%url, "Fetching");
    let res = reqwest::Client::new()
        .get(url.clone())
        .send()
        .await
        .with_context(|| format!("Fetching `{}`", url))?
        .error_for_status()
        .with_context(|| format!("Fetching `{}`", url))?;
    let bytes = res
        .bytes()
        .await
        .with_context(|| format!("Fetching `{}`", url))?;
    Ok(bytes.to_vec())
}

pub(crate) async fn fetch_string(url: &Url) -> anyhow::Result<String> {
    let bytes = fetch_bytes(url).await?;
    String::from_utf8(bytes).context("Output was not valid UTF-8")
}

/// The lower case hex SHA-256 of `bytes`, as `sha256sum` prints it
pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;

    sha2::Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Unpack a `.tar.gz` into `dest`, creating it if needed
pub(crate) async fn unpack_tar_gz(bytes: Vec<u8>, dest: &Path) -> anyhow::Result<()> {
    tokio::fs::create_dir_all(dest)
        .await
        .with_context(|| format!("Creating directory `{}`", dest.display()))?;

    let dest = dest.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let decoder = flate2::read::GzDecoder::new(std::io::Cursor::new(bytes));
        let mut archive = tar::Archive::new(decoder);
        archive.set_preserve_permissions(true);
        archive
            .unpack(&dest)
            .with_context(|| format!("Unpacking archive into `{}`", dest.display()))
    })
    .await
    .context("Joining a blocking task")?
}

/// `cp -a src/. dst/`
pub(crate) async fn copy_dir_all(src: &Path, dst: &Path) -> anyhow::Result<()> {
    let (src, dst) = (src.to_path_buf(), dst.to_path_buf());
    tokio::task::spawn_blocking(move || {
        for entry in WalkDir::new(&src).sort_by_file_name() {
            let entry = entry.with_context(|| format!("Walking directory `{}`", src.display()))?;
            let relative = entry.path().strip_prefix(&src).with_context(|| {
                format!(
                    "`{}` is not below `{}`",
                    entry.path().display(),
                    src.display()
                )
            })?;
            let target = dst.join(relative);

            if entry.file_type().is_dir() {
                std::fs::create_dir_all(&target)
                    .with_context(|| format!("Creating directory `{}`", target.display()))?;
            } else {
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)
                        .with_context(|| format!("Creating directory `{}`", parent.display()))?;
                }
                std::fs::copy(entry.path(), &target).with_context(|| {
                    format!(
                        "Copying `{}` to `{}`",
                        entry.path().display(),
                        target.display()
                    )
                })?;
                let mode = entry
                    .metadata()
                    .with_context(|| format!("Walking directory `{}`", src.display()))?
                    .permissions()
                    .mode();
                std::fs::set_permissions(&target, PermissionsExt::from_mode(mode)).with_context(
                    || format!("Setting mode `{:#o}` on `{}`", mode, target.display()),
                )?;
            }
        }
        Ok(())
    })
    .await
    .context("Joining a blocking task")?
}

/// The distribution codename, read from `/etc/os-release` rather than from `lsb_release`,
/// which may not be installed when the plan is made
pub(crate) async fn os_release_codename() -> anyhow::Result<String> {
    let path = Path::new("/etc/os-release");
    let contents = tokio::fs::read_to_string(path)
        .await
        .with_context(|| format!("Reading `{}`", path.display()))?;

    for line in contents.lines() {
        if let Some(value) = line.strip_prefix("VERSION_CODENAME=") {
            return Ok(value.trim_matches('"').to_string());
        }
    }

    anyhow::bail!("`{}` has no VERSION_CODENAME", path.display())
}

/// Percent encode a value for use inside a URL (`urllib.parse.quote(safe='')`)
pub(crate) fn urlencode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            },
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// Quote a value for the right hand side of a systemd `EnvironmentFile` assignment
pub(crate) fn systemd_env_quote(value: &str) -> String {
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

/// Escape a field of a `.pgpass` line
pub(crate) fn pgpass_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace(':', "\\:")
}

/// Escape a string literal for a SQL statement
pub(crate) fn sql_escape(value: &str) -> String {
    value.replace('\'', "''")
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn urlencodes_reserved_characters() {
        assert_eq!(urlencode("p@ss:w/rd"), "p%40ss%3Aw%2Frd");
        assert_eq!(urlencode("plain-value_1.0~"), "plain-value_1.0~");
    }

    #[test]
    fn escapes_pgpass_fields() {
        assert_eq!(pgpass_escape("a:b\\c"), "a\\:b\\\\c");
    }

    #[test]
    fn quotes_systemd_environment_values() {
        assert_eq!(systemd_env_quote("a\"b"), "\"a\\\"b\"");
    }

    #[test]
    fn hashes_like_sha256sum() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[tokio::test]
    async fn atomic_write_sets_the_mode_and_leaves_no_temporary() -> std::io::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("secret");

        write_atomic(&path, "hunter2", 0o600, None).await?;

        assert_eq!(tokio::fs::read_to_string(&path).await?, "hunter2");
        assert_eq!(
            tokio::fs::metadata(&path).await?.permissions().mode() & 0o777,
            0o600
        );
        assert!(!path.with_extension("tmp").exists());
        assert!(file_has_contents(&path).await);
        assert!(!file_has_contents(dir.path()).await);
        assert!(directory_has_contents(dir.path()).await);
        Ok(())
    }
}
