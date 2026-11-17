//! Owner-only filesystem helpers for health data: the SQLite DB, its
//! `-wal` / `-shm` / `-journal` sidecars, and `--out` exports.
//!
//! Files are created with mode 0600 in the `open(2)` call itself
//! (`OpenOptionsExt::mode`), so there is no window where a freshly created
//! file is world-readable and then `chmod`ed. New directories are created
//! with mode 0700. The process umask can only narrow these further.

use std::fs;
use std::io::{self, Write};
use std::path::Path;

/// Mode for files that hold health data.
pub const PRIVATE_FILE_MODE: u32 = 0o600;
/// Mode for directories this tool creates to hold health data.
pub const PRIVATE_DIR_MODE: u32 = 0o700;

/// Create `dir` and any missing parents with mode 0700.
///
/// Directories that already exist are left as they are: they may be shared
/// on purpose (for example a project folder), and narrowing them is the
/// owner's call.
pub fn create_private_dir_all(dir: &Path) -> io::Result<()> {
    if dir.as_os_str().is_empty() {
        return Ok(());
    }
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(PRIVATE_DIR_MODE);
    }
    builder.create(dir)
}

/// Open `path` for writing, creating it with mode 0600 if missing. A file
/// that already existed with wider bits is narrowed to 0600 through the open
/// descriptor (`fchmod`) before any new bytes are written.
fn open_private(path: &Path, truncate: bool) -> io::Result<fs::File> {
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create(true).truncate(truncate);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(PRIVATE_FILE_MODE);
    }
    let file = opts.open(path)?;
    restrict_open_file(&file)?;
    Ok(file)
}

#[cfg(unix)]
fn restrict_open_file(file: &fs::File) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mode = file.metadata()?.permissions().mode() & 0o7777;
    if mode != PRIVATE_FILE_MODE {
        file.set_permissions(fs::Permissions::from_mode(PRIVATE_FILE_MODE))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn restrict_open_file(_file: &fs::File) -> io::Result<()> {
    Ok(())
}

/// Make sure `path` exists as a 0600 file without touching its contents.
pub fn ensure_private_file(path: &Path) -> io::Result<()> {
    open_private(path, false).map(drop)
}

/// Narrow an existing file to 0600. A missing file is not an error.
pub fn restrict_if_exists(path: &Path) -> io::Result<()> {
    match fs::OpenOptions::new().read(true).open(path) {
        Ok(file) => restrict_open_file(&file),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Write `contents` to `path` as a 0600 file, creating missing parent
/// directories as 0700. Use this for any export that can hold health values.
pub fn write_private(path: &Path, contents: impl AsRef<[u8]>) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        create_private_dir_all(parent)?;
    }
    let mut file = open_private(path, true)?;
    file.write_all(contents.as_ref())?;
    file.flush()
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::TempDir;

    fn mode(path: &Path) -> u32 {
        fs::metadata(path).unwrap().permissions().mode() & 0o7777
    }

    #[test]
    fn write_private_creates_0600_file_and_0700_parents() {
        let dir = TempDir::new().unwrap();
        let out = dir.path().join("reports/2026/brief.md");
        write_private(&out, "resting_hr = 52.00 bpm\n").unwrap();
        assert_eq!(mode(&out), 0o600);
        assert_eq!(mode(&dir.path().join("reports")), 0o700);
        assert_eq!(mode(&dir.path().join("reports/2026")), 0o700);
        assert_eq!(
            fs::read_to_string(&out).unwrap(),
            "resting_hr = 52.00 bpm\n"
        );
    }

    #[test]
    fn write_private_narrows_an_existing_world_readable_file() {
        let dir = TempDir::new().unwrap();
        let out = dir.path().join("brief.md");
        fs::write(&out, "old").unwrap();
        fs::set_permissions(&out, fs::Permissions::from_mode(0o644)).unwrap();
        write_private(&out, "new").unwrap();
        assert_eq!(mode(&out), 0o600);
        assert_eq!(fs::read_to_string(&out).unwrap(), "new");
    }

    #[test]
    fn ensure_private_file_keeps_contents_and_restrict_ignores_missing() {
        let dir = TempDir::new().unwrap();
        let f = dir.path().join("data.db");
        fs::write(&f, "keep").unwrap();
        fs::set_permissions(&f, fs::Permissions::from_mode(0o664)).unwrap();
        ensure_private_file(&f).unwrap();
        assert_eq!(mode(&f), 0o600);
        assert_eq!(fs::read_to_string(&f).unwrap(), "keep");
        restrict_if_exists(&dir.path().join("absent-wal")).unwrap();
    }
}
