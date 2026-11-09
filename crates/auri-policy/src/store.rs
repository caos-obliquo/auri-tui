//! The XDG data directory and the private-file helpers every state file uses.

use std::path::{Path, PathBuf};

/// `~/.local/share/auri` (or `$XDG_DATA_HOME/auri`), where `flash.json` and
/// `journal.jsonl` live.
#[must_use]
pub fn data_dir() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_DATA_HOME")
        && !xdg.is_empty()
    {
        return PathBuf::from(xdg).join("auri");
    }
    if let Ok(home) = std::env::var("HOME")
        && !home.is_empty()
    {
        return PathBuf::from(home)
            .join(".local")
            .join("share")
            .join("auri");
    }
    // No HOME at all (a bare container): stay inside the working directory
    // rather than inventing a home. The counters survive for that process.
    PathBuf::from(".auri")
}

/// The file mode private state is created with (owner read/write only).
#[cfg(unix)]
const PRIVATE_MODE: u32 = 0o600;

fn open_options() -> std::fs::OpenOptions {
    let mut options = std::fs::OpenOptions::new();
    options.create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(PRIVATE_MODE);
    }
    options
}

fn ensure_private(path: &Path) {
    // `mode` only applies when the file is created; an existing file keeps
    // whatever mode it has, so tighten it explicitly. Best effort: the data
    // itself was still written, and a chmod failure is not worth a retry loop.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(PRIVATE_MODE));
    }
    let _ = path;
}

/// Replaces `path` with `bytes`, creating the parent directory and keeping the
/// file owner-only.
///
/// # Errors
///
/// Whatever the filesystem reports.
pub fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut options = open_options();
    options.write(true).truncate(true);
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    drop(file);
    ensure_private(path);
    Ok(())
}

/// Appends `bytes` to `path`, creating the parent directory and keeping the
/// file owner-only. The journal's durability comes from appending whole lines,
/// never from rewriting what is already there.
///
/// # Errors
///
/// Whatever the filesystem reports.
pub fn append_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut options = open_options();
    options.append(true);
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    drop(file);
    ensure_private(path);
    Ok(())
}
