//! Rollback-mode snapshot of the live health database for the warehouse loader.
//!
//! The live database stays in `journal_mode=WAL`. The snapshot is built with
//! `VACUUM INTO` in a private directory, then published through the inbox
//! directory fd (`openat`, `renameat`, `fsync`). The inbox path is not used
//! again after that directory is opened.

use std::path::{Path, PathBuf};

#[cfg(not(target_os = "linux"))]
use crate::error::GroktorError;
use crate::error::Result;

/// Publish a consistent snapshot of `live_db` as `inbox/data.db`.
///
/// `inbox` must already exist and must not be a symlink. `inbox/data.db`,
/// when it already exists, must be a regular file. The previous `data.db` is
/// left in place if anything fails before `renameat`.
pub fn publish_snapshot(live_db: &Path, inbox: &Path) -> Result<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        linux::publish(live_db, inbox)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (live_db, inbox);
        Err(GroktorError::Message(
            "warehouse snapshot export requires Linux".into(),
        ))
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use std::ffi::{CStr, CString, OsStr};
    use std::fs::{self, File, OpenOptions};
    use std::io::{self, Read, Seek};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
    use std::os::unix::io::{AsRawFd, FromRawFd};
    use std::path::{Path, PathBuf};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use rusqlite::{Connection, ErrorCode, OpenFlags};
    use uuid::Uuid;

    use crate::error::{GroktorError, Result};

    /// Group-readable, owner-writable. The warehouse inbox is mode `3750`
    /// (setgid and sticky): a new file inherits the inbox group, so the loader
    /// can read it, and the sticky bit keeps other users from unlinking it.
    /// World has no bits. `fchmod` applies this even when umask is wider.
    const SNAPSHOT_MODE: u32 = 0o640;
    const PRIVATE_DIR_MODE: u32 = 0o700;
    /// Linux `O_NOFOLLOW`.
    const O_NOFOLLOW: i32 = 0o400000;
    /// Linux `O_DIRECTORY`.
    const O_DIRECTORY: i32 = 0o200000;
    const SIDECAR_SUFFIXES: [&str; 3] = ["-wal", "-shm", "-journal"];
    /// Whole-export bound. A busy writer must not make this run longer, and
    /// the previous `data.db` is not touched when the bound is hit.
    const EXPORT_DEADLINE: Duration = Duration::from_secs(60);

    struct OpenedInbox {
        /// Directory fd. Every inbox step after `open_inbox` uses this, not the path.
        dir: File,
    }

    struct PrivateDir {
        path: PathBuf,
    }

    impl PrivateDir {
        fn create() -> Result<Self> {
            let path =
                std::env::temp_dir().join(format!("groktor-export-{}", Uuid::new_v4().as_simple()));
            let c_path = c_path(&path)?;
            let rc = unsafe { libc::mkdir(c_path.as_ptr(), PRIVATE_DIR_MODE) };
            if rc != 0 {
                return Err(GroktorError::Message(format!(
                    "failed to create a private snapshot directory: {}",
                    io::Error::last_os_error()
                )));
            }
            let dir = OpenOptions::new()
                .read(true)
                .custom_flags(O_NOFOLLOW | O_DIRECTORY)
                .open(&path)
                .map_err(|err| {
                    let _ = fs::remove_dir(&path);
                    GroktorError::Message(format!(
                        "failed to open the private snapshot directory: {err}"
                    ))
                })?;
            if let Err(err) = dir.set_permissions(fs::Permissions::from_mode(PRIVATE_DIR_MODE)) {
                drop(dir);
                let _ = fs::remove_dir(&path);
                return Err(GroktorError::Message(format!(
                    "failed to restrict the private snapshot directory: {err}"
                )));
            }
            let mode = match dir.metadata() {
                Ok(meta) => meta.permissions().mode() & 0o777,
                Err(err) => {
                    drop(dir);
                    let _ = fs::remove_dir(&path);
                    return Err(GroktorError::Message(format!(
                        "failed to stat the private snapshot directory: {err}"
                    )));
                }
            };
            drop(dir);
            if mode & !PRIVATE_DIR_MODE != 0 {
                let _ = fs::remove_dir(&path);
                return Err(GroktorError::Message(format!(
                    "private snapshot directory mode is {mode:04o}; expected 0700 or tighter"
                )));
            }
            Ok(Self { path })
        }
    }

    impl Drop for PrivateDir {
        fn drop(&mut self) {
            match fs::symlink_metadata(&self.path) {
                Ok(meta) if meta.file_type().is_symlink() => {
                    let _ = fs::remove_file(&self.path);
                }
                Ok(meta) if meta.is_dir() => {
                    let _ = fs::remove_dir_all(&self.path);
                }
                _ => {}
            }
        }
    }

    /// Inbox temp name created with `openat`. Drop unlinks it unless `renameat`
    /// has already consumed the name.
    struct InboxTemp<'a> {
        dir: &'a File,
        name: CString,
        armed: bool,
    }

    impl Drop for InboxTemp<'_> {
        fn drop(&mut self) {
            if self.armed {
                let _ = unlink_name(self.dir, &self.name);
            }
        }
    }

    pub(super) fn publish(live_db: &Path, inbox: &Path) -> Result<PathBuf> {
        let opened = open_inbox(inbox)?;
        let dest = inbox.join("data.db");
        if same_lexical_path(&dest, live_db) {
            return Err(GroktorError::Message(format!(
                "refusing to replace the live database ({}) with the snapshot",
                live_db.display()
            )));
        }
        publish_into(&opened, live_db)?;
        Ok(dest)
    }

    fn publish_into(inbox: &OpenedInbox, live_db: &Path) -> Result<()> {
        let started = Instant::now();
        check_deadline(started)?;
        require_regular_target(&inbox.dir, live_db)?;
        refuse_odd_sidecars(&inbox.dir)?;

        let private = PrivateDir::create()?;
        let snapshot = private.path.join("snapshot.db");
        vacuum_into(live_db, &snapshot, started)?;
        finalize_private_snapshot(&snapshot)?;
        check_deadline(started)?;

        require_regular_target(&inbox.dir, live_db)?;
        refuse_odd_sidecars(&inbox.dir)?;

        let (tmp_c, mut tmp_file) = create_inbox_temp(&inbox.dir)?;
        let mut guard = InboxTemp {
            dir: &inbox.dir,
            name: tmp_c,
            armed: true,
        };
        copy_private_into_fd(&snapshot, &mut tmp_file)?;
        tmp_file.sync_all().map_err(|err| {
            GroktorError::Message(format!("failed to fsync the inbox temp file: {err}"))
        })?;
        ensure_header_file(&mut tmp_file)?;
        ensure_mode_fd(&tmp_file)?;
        integrity_fd(&tmp_file)?;
        check_deadline(started)?;
        require_regular_target(&inbox.dir, live_db)?;
        refuse_odd_sidecars(&inbox.dir)?;
        drop(tmp_file);

        rename_over_data_db(&inbox.dir, &guard.name)?;
        guard.armed = false;
        remove_file_sidecars(&inbox.dir)?;
        inbox.dir.sync_all().map_err(|err| {
            GroktorError::Message(format!(
                "snapshot was renamed onto data.db but the inbox directory could not be fsynced: {err}"
            ))
        })?;
        Ok(())
    }

    fn open_inbox(inbox: &Path) -> Result<OpenedInbox> {
        if inbox.as_os_str().is_empty() {
            return Err(GroktorError::Message("export inbox path is empty".into()));
        }
        let listed = match fs::symlink_metadata(inbox) {
            Ok(meta) => meta,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                return Err(GroktorError::Message(format!(
                    "export inbox does not exist: {}",
                    inbox.display()
                )));
            }
            Err(err) => {
                return Err(GroktorError::Message(format!(
                    "failed to inspect export inbox {}: {err}",
                    inbox.display()
                )));
            }
        };
        if listed.file_type().is_symlink() {
            return Err(GroktorError::Message(format!(
                "export inbox is a symlink ({}); refusing to follow it",
                inbox.display()
            )));
        }
        if !listed.is_dir() {
            return Err(GroktorError::Message(format!(
                "export inbox is not a directory: {}",
                inbox.display()
            )));
        }
        let dir = OpenOptions::new()
            .read(true)
            .custom_flags(O_NOFOLLOW | O_DIRECTORY)
            .open(inbox)
            .map_err(|err| {
                GroktorError::Message(format!(
                    "failed to open export inbox {} without following a symlink: {err}",
                    inbox.display()
                ))
            })?;
        let opened = dir.metadata().map_err(|err| {
            GroktorError::Message(format!(
                "failed to stat export inbox {}: {err}",
                inbox.display()
            ))
        })?;
        if opened.dev() != listed.dev() || opened.ino() != listed.ino() || !opened.is_dir() {
            return Err(GroktorError::Message(format!(
                "export inbox {} changed while opening; refusing to follow it",
                inbox.display()
            )));
        }
        Ok(OpenedInbox { dir })
    }

    fn same_lexical_path(dest: &Path, live: &Path) -> bool {
        match (std::path::absolute(dest), std::path::absolute(live)) {
            (Ok(dest), Ok(live)) => dest == live,
            _ => false,
        }
    }

    fn require_regular_target(dir: &File, live_db: &Path) -> Result<()> {
        match fstat_name(dir, c"data.db")? {
            None => Ok(()),
            Some(st) if is_lnk(&st) => Err(GroktorError::Message(
                "export target is a symlink (data.db); refusing to follow it".into(),
            )),
            Some(st) if !is_reg(&st) => Err(GroktorError::Message(
                "export target exists and is not a regular file: data.db".into(),
            )),
            Some(st) => {
                if let Ok(live) = fs::metadata(live_db) {
                    if live.dev() == st.st_dev && live.ino() == st.st_ino {
                        return Err(GroktorError::Message(format!(
                            "refusing to replace the live database ({}) with the snapshot",
                            live_db.display()
                        )));
                    }
                }
                Ok(())
            }
        }
    }

    /// A directory (or other non-file) sidecar cannot be unlinked with
    /// `unlinkat`. Fail before `renameat` so `data.db` stays as it was.
    fn refuse_odd_sidecars(dir: &File) -> Result<()> {
        for suffix in SIDECAR_SUFFIXES {
            let name = sidecar_cstr(suffix);
            match fstat_name(dir, &name)? {
                None => {}
                Some(st) if is_reg(&st) || is_lnk(&st) => {}
                Some(_) => {
                    return Err(GroktorError::Message(format!(
                        "sidecar data.db{suffix} is not a regular file; refusing to publish"
                    )));
                }
            }
        }
        Ok(())
    }

    fn remove_file_sidecars(dir: &File) -> Result<()> {
        for suffix in SIDECAR_SUFFIXES {
            let name = sidecar_cstr(suffix);
            match fstat_name(dir, &name)? {
                None => {}
                Some(st) if is_reg(&st) || is_lnk(&st) => {
                    unlink_name(dir, &name).map_err(|err| {
                        GroktorError::Message(format!(
                            "failed to remove sidecar data.db{suffix} without following it: {err}"
                        ))
                    })?;
                }
                Some(_) => {
                    return Err(GroktorError::Message(format!(
                        "sidecar data.db{suffix} is not a regular file; left it in place"
                    )));
                }
            }
        }
        Ok(())
    }

    fn vacuum_into(live_db: &Path, dest: &Path, started: Instant) -> Result<()> {
        check_deadline(started)?;
        let dest_text = dest.to_str().ok_or_else(|| {
            GroktorError::Message(
                "private snapshot path is not valid UTF-8, so VACUUM INTO cannot name it".into(),
            )
        })?;
        let src = Connection::open_with_flags(live_db, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(
            |err| {
                GroktorError::Message(format!(
                    "failed to open the live database read-only ({}): {err}",
                    live_db.display()
                ))
            },
        )?;
        let left = EXPORT_DEADLINE.saturating_sub(started.elapsed());
        src.busy_timeout(left.min(Duration::from_secs(5)))?;
        let interrupt = src.get_interrupt_handle();
        let (stop_tx, stop_rx) = mpsc::channel();
        let watchdog = std::thread::spawn(move || {
            if stop_rx.recv_timeout(left).is_err() {
                interrupt.interrupt();
            }
        });
        let vacuumed = src.execute("VACUUM INTO ?1", [dest_text]);
        let _ = stop_tx.send(());
        let _ = watchdog.join();
        match vacuumed {
            Ok(_) => Ok(()),
            Err(err) if is_interrupt(&err) => Err(deadline_error()),
            Err(err) => Err(GroktorError::Message(format!(
                "failed to snapshot the live database ({}): {err}",
                live_db.display()
            ))),
        }
    }

    fn finalize_private_snapshot(path: &Path) -> Result<()> {
        let conn = Connection::open(path).map_err(|err| {
            GroktorError::Message(format!(
                "failed to open the private snapshot {}: {err}",
                path.display()
            ))
        })?;
        let mode: String = conn.query_row("PRAGMA journal_mode = DELETE", [], |row| row.get(0))?;
        if !mode.eq_ignore_ascii_case("delete") {
            return Err(GroktorError::Message(format!(
                "snapshot journal_mode stayed {mode}; rollback mode (DELETE) is required"
            )));
        }
        let messages = integrity_messages(&conn)?;
        if messages != ["ok"] {
            return Err(GroktorError::Message(format!(
                "snapshot integrity_check failed: {}",
                messages.join("; ")
            )));
        }
        conn.close().map_err(|(_, err)| {
            GroktorError::Message(format!(
                "failed to close the private snapshot {}: {err}",
                path.display()
            ))
        })?;
        for suffix in SIDECAR_SUFFIXES {
            let side = sidecar_path(path, suffix);
            match fs::symlink_metadata(&side) {
                Err(err) if err.kind() == io::ErrorKind::NotFound => {}
                Err(err) => {
                    return Err(GroktorError::Message(format!(
                        "failed to inspect private sidecar {}: {err}",
                        side.display()
                    )));
                }
                Ok(_) => {
                    return Err(GroktorError::Message(format!(
                        "private snapshot left sidecar {}; refusing to publish",
                        side.display()
                    )));
                }
            }
        }
        let bytes = fs::read(path).map_err(|err| {
            GroktorError::Message(format!(
                "failed to read the private snapshot {}: {err}",
                path.display()
            ))
        })?;
        ensure_header_bytes(&bytes, path)?;
        Ok(())
    }

    fn create_inbox_temp(dir: &File) -> Result<(CString, File)> {
        for _ in 0..8 {
            let name = CString::new(temp_file_name()).map_err(|_| {
                GroktorError::Message("snapshot temp name contains a NUL byte".into())
            })?;
            match openat_excl(dir, &name) {
                Ok(file) => {
                    if let Err(err) =
                        file.set_permissions(fs::Permissions::from_mode(SNAPSHOT_MODE))
                    {
                        drop(file);
                        let _ = unlink_name(dir, &name);
                        return Err(GroktorError::Message(format!(
                            "failed to set the inbox temp file mode: {err}"
                        )));
                    }
                    if let Err(err) = ensure_mode_fd(&file) {
                        drop(file);
                        let _ = unlink_name(dir, &name);
                        return Err(err);
                    }
                    return Ok((name, file));
                }
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(err) => {
                    return Err(GroktorError::Message(format!(
                        "failed to create snapshot temp file in the inbox: {err}"
                    )));
                }
            }
        }
        Err(GroktorError::Message(
            "failed to create a unique snapshot temp file after 8 attempts".into(),
        ))
    }

    fn copy_private_into_fd(src_path: &Path, dest: &mut File) -> Result<()> {
        let mut src = OpenOptions::new()
            .read(true)
            .custom_flags(O_NOFOLLOW)
            .open(src_path)
            .map_err(|err| {
                GroktorError::Message(format!(
                    "failed to read the private snapshot {}: {err}",
                    src_path.display()
                ))
            })?;
        let expected = src
            .metadata()
            .map_err(|err| {
                GroktorError::Message(format!("failed to stat the private snapshot: {err}"))
            })?
            .len();
        let copied = io::copy(&mut src, dest).map_err(|err| {
            GroktorError::Message(format!(
                "failed to copy the snapshot into the inbox temp file: {err}"
            ))
        })?;
        if copied != expected {
            return Err(GroktorError::Message(format!(
                "inbox temp file received {copied} bytes of a {expected} byte snapshot"
            )));
        }
        Ok(())
    }

    fn rename_over_data_db(dir: &File, tmp_name: &CStr) -> Result<()> {
        let rc = unsafe {
            libc::renameat(
                dir.as_raw_fd(),
                tmp_name.as_ptr(),
                dir.as_raw_fd(),
                c"data.db".as_ptr(),
            )
        };
        if rc == 0 {
            Ok(())
        } else {
            Err(GroktorError::Message(format!(
                "failed to rename snapshot onto data.db (existing data.db was left unchanged): {}",
                io::Error::last_os_error()
            )))
        }
    }

    fn integrity_fd(file: &File) -> Result<()> {
        let uri = format!("file:/proc/self/fd/{}?immutable=1", file.as_raw_fd());
        let conn = Connection::open_with_flags(
            &uri,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
        )
        .map_err(|err| {
            GroktorError::Message(format!(
                "failed to open the inbox snapshot for integrity_check: {err}"
            ))
        })?;
        let messages = integrity_messages(&conn)?;
        if messages != ["ok"] {
            return Err(GroktorError::Message(format!(
                "snapshot integrity_check failed: {}",
                messages.join("; ")
            )));
        }
        Ok(())
    }

    fn integrity_messages(conn: &Connection) -> Result<Vec<String>> {
        let mut stmt = conn.prepare("PRAGMA integrity_check")?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        let mut messages = Vec::new();
        for row in rows {
            messages.push(row?);
        }
        Ok(messages)
    }

    fn ensure_header_file(file: &mut File) -> Result<()> {
        file.rewind().map_err(|err| {
            GroktorError::Message(format!("failed to rewind the inbox snapshot: {err}"))
        })?;
        let mut hdr = [0u8; 20];
        file.read_exact(&mut hdr).map_err(|err| {
            GroktorError::Message(format!(
                "inbox snapshot is too small to be a SQLite database: {err}"
            ))
        })?;
        ensure_header_bytes(&hdr, Path::new("data.db"))
    }

    fn ensure_header_bytes(bytes: &[u8], path: &Path) -> Result<()> {
        if bytes.len() < 20 || &bytes[0..16] != b"SQLite format 3\0" {
            return Err(GroktorError::Message(format!(
                "snapshot {} is not a SQLite database",
                path.display()
            )));
        }
        if bytes[18] != 1 || bytes[19] != 1 {
            return Err(GroktorError::Message(format!(
                "snapshot is not in rollback journal mode (header bytes 18 and 19 are {} and {}, expected 1 and 1)",
                bytes[18], bytes[19]
            )));
        }
        Ok(())
    }

    fn ensure_mode_fd(file: &File) -> Result<()> {
        let mode = file
            .metadata()
            .map_err(|err| {
                GroktorError::Message(format!("failed to stat the inbox snapshot: {err}"))
            })?
            .permissions()
            .mode()
            & 0o777;
        if mode != SNAPSHOT_MODE {
            return Err(GroktorError::Message(format!(
                "snapshot mode is {mode:04o}; expected 0640"
            )));
        }
        Ok(())
    }

    fn check_deadline(started: Instant) -> Result<()> {
        if started.elapsed() >= EXPORT_DEADLINE {
            Err(deadline_error())
        } else {
            Ok(())
        }
    }

    fn deadline_error() -> GroktorError {
        GroktorError::Message(
            "snapshot export did not finish within 60s; the previous data.db was left unchanged"
                .into(),
        )
    }

    fn is_interrupt(err: &rusqlite::Error) -> bool {
        matches!(
            err,
            rusqlite::Error::SqliteFailure(code, _)
                if code.code == ErrorCode::OperationInterrupted
        )
    }

    fn temp_file_name() -> String {
        format!(".data.db.{}.tmp", Uuid::new_v4().as_simple())
    }

    fn sidecar_path(db: &Path, suffix: &str) -> PathBuf {
        let mut name = db.as_os_str().to_os_string();
        name.push(suffix);
        PathBuf::from(name)
    }

    fn sidecar_cstr(suffix: &str) -> CString {
        CString::new(format!("data.db{suffix}")).expect("sidecar name has no NUL")
    }

    fn c_path(path: &Path) -> Result<CString> {
        CString::new(OsStr::as_bytes(path.as_os_str())).map_err(|_| {
            GroktorError::Message(format!("path {} contains a NUL byte", path.display()))
        })
    }

    fn fstat_name(dir: &File, name: &CStr) -> Result<Option<libc::stat>> {
        let mut st = std::mem::MaybeUninit::<libc::stat>::zeroed();
        let rc = unsafe {
            libc::fstatat(
                dir.as_raw_fd(),
                name.as_ptr(),
                st.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if rc == 0 {
            Ok(Some(unsafe { st.assume_init() }))
        } else {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::NotFound {
                Ok(None)
            } else {
                Err(GroktorError::Message(format!(
                    "failed to inspect inbox entry: {err}"
                )))
            }
        }
    }

    fn openat_excl(dir: &File, name: &CStr) -> io::Result<File> {
        let fd = unsafe {
            libc::openat(
                dir.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDWR | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                SNAPSHOT_MODE as libc::mode_t,
            )
        };
        if fd < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(unsafe { File::from_raw_fd(fd) })
        }
    }

    fn unlink_name(dir: &File, name: &CStr) -> io::Result<()> {
        let rc = unsafe { libc::unlinkat(dir.as_raw_fd(), name.as_ptr(), 0) };
        if rc == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    fn is_reg(st: &libc::stat) -> bool {
        st.st_mode & libc::S_IFMT == libc::S_IFREG
    }

    fn is_lnk(st: &libc::stat) -> bool {
        st.st_mode & libc::S_IFMT == libc::S_IFLNK
    }

    #[cfg(test)]
    mod tests {
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::sync::Arc;

        use chrono::{NaiveDate, Utc};
        use rusqlite::{Connection, OpenFlags};

        use super::*;
        use crate::export::publish_snapshot;
        use crate::schema::{MetricKind, MetricPoint};
        use crate::store::Store;

        fn mode_of(path: &Path) -> u32 {
            fs::metadata(path).unwrap().permissions().mode() & 0o777
        }

        fn dot_tmps(dir: &Path) -> Vec<String> {
            let mut names = Vec::new();
            for entry in fs::read_dir(dir).unwrap() {
                let name = entry.unwrap().file_name();
                let name = name.to_string_lossy();
                if name.starts_with('.') && name.ends_with(".tmp") {
                    names.push(name.into_owned());
                }
            }
            names
        }

        fn sqlite_uri(path: &Path) -> String {
            let text = path.to_string_lossy();
            let mut encoded = String::from("file:");
            for byte in text.bytes() {
                match byte {
                    b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'.' | b'_' | b'-' => {
                        encoded.push(byte as char);
                    }
                    _ => encoded.push_str(&format!("%{byte:02X}")),
                }
            }
            encoded.push_str("?immutable=1");
            encoded
        }

        fn open_immutable(path: &Path) -> Connection {
            Connection::open_with_flags(
                sqlite_uri(path),
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
            )
            .unwrap_or_else(|err| panic!("immutable open {}: {err}", path.display()))
        }

        fn metric_ids(conn: &Connection) -> Vec<String> {
            let mut stmt = conn.prepare("SELECT id FROM metrics ORDER BY id").unwrap();
            let rows = stmt.query_map([], |row| row.get::<_, String>(0)).unwrap();
            rows.map(|row| row.unwrap()).collect()
        }

        fn header_pair(path: &Path) -> (u8, u8) {
            let bytes = fs::read(path).unwrap();
            assert!(bytes.len() > 20, "{}", path.display());
            assert_eq!(&bytes[..16], b"SQLite format 3\0");
            (bytes[18], bytes[19])
        }

        fn assert_published_ids(inbox: &Path, mut expected: Vec<String>) {
            let dest = inbox.join("data.db");
            let conn = open_immutable(&dest);
            let got = metric_ids(&conn);
            drop(conn);
            expected.sort();
            let mut got_sorted = got;
            got_sorted.sort();
            assert_eq!(got_sorted, expected);
            assert_eq!(header_pair(&dest), (1, 1));
            assert_eq!(mode_of(&dest), 0o640);
            for suffix in SIDECAR_SUFFIXES {
                assert!(
                    !sidecar_path(&dest, suffix).exists(),
                    "sidecar {suffix} left beside {}",
                    dest.display()
                );
            }
            assert!(dot_tmps(inbox).is_empty(), "{:?}", dot_tmps(inbox));
        }

        fn insert_point(store: &Store, value: f64) -> MetricPoint {
            let day = NaiveDate::from_ymd_opt(2026, 8, 1).unwrap();
            let point = MetricPoint::new(
                MetricKind::Steps,
                Utc::now(),
                day,
                value,
                "count",
                "export-test",
            );
            store.upsert_metrics(std::slice::from_ref(&point)).unwrap();
            point
        }

        #[test]
        fn temp_name_is_hidden_and_ends_with_tmp() {
            let name = temp_file_name();
            assert!(name.starts_with(".data.db."), "{name}");
            assert!(name.ends_with(".tmp"), "{name}");
            assert!(!name.contains('/'), "{name}");
            let body = name
                .strip_prefix(".data.db.")
                .and_then(|rest| rest.strip_suffix(".tmp"))
                .unwrap_or_else(|| panic!("{name}"));
            assert_eq!(body.len(), 32, "{name}");
            assert!(body.chars().all(|c| c.is_ascii_hexdigit()), "{name}");
        }

        #[test]
        fn private_snapshot_dir_is_owner_only() {
            let dir = PrivateDir::create().unwrap();
            assert_eq!(mode_of(&dir.path), 0o700);
        }

        #[test]
        fn exclusive_create_sets_mode_and_does_not_follow_a_symlink() {
            let dir = tempfile::TempDir::new().unwrap();
            let opened = open_inbox(dir.path()).unwrap();
            let target = dir.path().join("target");
            fs::write(&target, b"keep").unwrap();
            let link_name = c".data.db.link.tmp";
            std::os::unix::fs::symlink(&target, dir.path().join(".data.db.link.tmp")).unwrap();
            let err = openat_excl(&opened.dir, link_name).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
            assert_eq!(fs::read(&target).unwrap(), b"keep");
            assert!(fs::symlink_metadata(dir.path().join(".data.db.link.tmp"))
                .unwrap()
                .file_type()
                .is_symlink());

            let fresh = CString::new(".data.db.fresh.tmp").unwrap();
            let file = openat_excl(&opened.dir, &fresh).unwrap();
            file.set_permissions(fs::Permissions::from_mode(SNAPSHOT_MODE))
                .unwrap();
            drop(file);
            assert_eq!(mode_of(&dir.path().join(".data.db.fresh.tmp")), 0o640);
            let again = openat_excl(&opened.dir, &fresh).unwrap_err();
            assert_eq!(again.kind(), io::ErrorKind::AlreadyExists);
        }

        #[test]
        fn snapshot_keeps_wal_only_rows_and_opens_immutable() {
            let dir = tempfile::TempDir::new().unwrap();
            let live = dir.path().join("live.db");
            let store = Store::open(&live).unwrap();
            let checkpointed = insert_point(&store, 1000.0);
            {
                let conn = Connection::open(&live).unwrap();
                conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
                    .unwrap();
            }
            let wal_only = insert_point(&store, 2000.0);

            let before = open_immutable(&live);
            let visible = metric_ids(&before);
            drop(before);
            assert!(
                visible.contains(&checkpointed.id.to_string()),
                "{visible:?}"
            );
            assert!(
                !visible.contains(&wal_only.id.to_string()),
                "row should still be only in the live -wal, saw {visible:?}"
            );
            let wal_path = sidecar_path(&live, "-wal");
            assert!(
                fs::metadata(&wal_path).unwrap().len() > 32,
                "expected a non-empty wal"
            );

            let inbox = dir.path().join("inbox");
            fs::create_dir(&inbox).unwrap();
            let dest = publish_snapshot(&live, &inbox).unwrap();
            assert_eq!(dest, inbox.join("data.db"));
            assert_published_ids(
                &inbox,
                vec![checkpointed.id.to_string(), wal_only.id.to_string()],
            );

            let after = open_immutable(&live);
            let still_main = metric_ids(&after);
            drop(after);
            assert!(
                !still_main.contains(&wal_only.id.to_string()),
                "export checkpointed the live database: {still_main:?}"
            );
            assert_eq!(header_pair(&live), (2, 2));
            let live_conn =
                Connection::open_with_flags(&live, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
            let journal: String = live_conn
                .query_row("PRAGMA journal_mode", [], |row| row.get(0))
                .unwrap();
            assert_eq!(journal, "wal");
            assert_eq!(store.metric_count().unwrap(), 2);
        }

        #[test]
        fn failed_export_leaves_previous_data_db_and_no_tmp() {
            let dir = tempfile::TempDir::new().unwrap();
            let inbox = dir.path().join("inbox");
            fs::create_dir(&inbox).unwrap();
            let dest = inbox.join("data.db");
            fs::write(&dest, b"OLD-SNAPSHOT").unwrap();
            fs::set_permissions(&dest, fs::Permissions::from_mode(0o600)).unwrap();

            let missing = dir.path().join("missing.db");
            let err = publish_snapshot(&missing, &inbox).unwrap_err();
            assert!(err.to_string().contains("live database"), "{err}");
            assert_eq!(fs::read(&dest).unwrap(), b"OLD-SNAPSHOT");
            assert_eq!(mode_of(&dest), 0o600);
            assert!(!missing.exists());
            assert!(dot_tmps(&inbox).is_empty(), "{:?}", dot_tmps(&inbox));
            for suffix in SIDECAR_SUFFIXES {
                assert!(!sidecar_path(&dest, suffix).exists());
            }

            let garbage = dir.path().join("not-a.db");
            fs::write(&garbage, b"this is not sqlite").unwrap();
            let err = publish_snapshot(&garbage, &inbox).unwrap_err();
            let text = err.to_string();
            assert!(
                text.contains("not a database")
                    || text.contains("live database")
                    || text.contains("snapshot"),
                "{text}"
            );
            assert_eq!(fs::read(&garbage).unwrap(), b"this is not sqlite");
            assert_eq!(fs::read(&dest).unwrap(), b"OLD-SNAPSHOT");
            assert!(dot_tmps(&inbox).is_empty(), "{:?}", dot_tmps(&inbox));

            let live = dir.path().join("live.db");
            let store = Store::open(&live).unwrap();
            let point = insert_point(&store, 7.0);
            publish_snapshot(&live, &inbox).unwrap();
            let published = fs::read(&dest).unwrap();
            assert_ne!(published, b"OLD-SNAPSHOT");
            assert_published_ids(&inbox, vec![point.id.to_string()]);

            let err = publish_snapshot(&garbage, &inbox).unwrap_err();
            assert!(!err.to_string().is_empty());
            assert_eq!(fs::read(&dest).unwrap(), published);
            assert_eq!(fs::read(&garbage).unwrap(), b"this is not sqlite");
            assert!(dot_tmps(&inbox).is_empty(), "{:?}", dot_tmps(&inbox));
        }

        #[test]
        fn symlinked_inbox_or_target_is_refused() {
            let dir = tempfile::TempDir::new().unwrap();
            let live = dir.path().join("live.db");
            let store = Store::open(&live).unwrap();
            let point = insert_point(&store, 3.0);

            let real = dir.path().join("real-inbox");
            fs::create_dir(&real).unwrap();
            let link = dir.path().join("inbox-link");
            std::os::unix::fs::symlink(&real, &link).unwrap();
            let err = publish_snapshot(&live, &link).unwrap_err();
            assert!(err.to_string().contains("symlink"), "{err}");
            assert!(!real.join("data.db").exists());
            assert!(dot_tmps(&real).is_empty());

            let inbox = dir.path().join("inbox");
            fs::create_dir(&inbox).unwrap();
            let secret = dir.path().join("secret");
            fs::write(&secret, b"do-not-touch").unwrap();
            std::os::unix::fs::symlink(&secret, inbox.join("data.db")).unwrap();
            let err = publish_snapshot(&live, &inbox).unwrap_err();
            assert!(err.to_string().contains("symlink"), "{err}");
            assert_eq!(fs::read(&secret).unwrap(), b"do-not-touch");
            assert!(fs::symlink_metadata(inbox.join("data.db"))
                .unwrap()
                .file_type()
                .is_symlink());
            assert!(dot_tmps(&inbox).is_empty(), "{:?}", dot_tmps(&inbox));

            fs::remove_file(inbox.join("data.db")).unwrap();
            let stale = dir.path().join("stale-wal");
            fs::write(&stale, b"wal-bytes").unwrap();
            std::os::unix::fs::symlink(&stale, inbox.join("data.db-wal")).unwrap();
            publish_snapshot(&live, &inbox).unwrap();
            assert_published_ids(&inbox, vec![point.id.to_string()]);
            assert!(!inbox.join("data.db-wal").exists());
            assert_eq!(fs::read(&stale).unwrap(), b"wal-bytes");
        }

        #[test]
        fn directory_sidecar_is_refused_before_replacing_data_db() {
            let dir = tempfile::TempDir::new().unwrap();
            let live = dir.path().join("live.db");
            let store = Store::open(&live).unwrap();
            insert_point(&store, 9.0);
            let inbox = dir.path().join("inbox");
            fs::create_dir(&inbox).unwrap();
            let dest = inbox.join("data.db");
            fs::write(&dest, b"KEEP").unwrap();
            fs::create_dir(inbox.join("data.db-wal")).unwrap();
            let err = publish_snapshot(&live, &inbox).unwrap_err();
            assert!(err.to_string().contains("sidecar"), "{err}");
            assert_eq!(fs::read(&dest).unwrap(), b"KEEP");
            assert!(inbox.join("data.db-wal").is_dir());
            assert!(dot_tmps(&inbox).is_empty(), "{:?}", dot_tmps(&inbox));
        }

        #[test]
        fn refuses_to_replace_the_live_database() {
            let dir = tempfile::TempDir::new().unwrap();
            let live = dir.path().join("data.db");
            let store = Store::open(&live).unwrap();
            insert_point(&store, 1.0);
            let before = fs::read(&live).unwrap();
            let err = publish_snapshot(&live, dir.path()).unwrap_err();
            assert!(err.to_string().contains("live database"), "{err}");
            assert_eq!(fs::read(&live).unwrap(), before);
            assert_eq!(header_pair(&live), (2, 2));
            assert!(dot_tmps(dir.path()).is_empty());
        }

        #[test]
        fn missing_or_non_directory_inbox_fails_before_a_temp_file() {
            let dir = tempfile::TempDir::new().unwrap();
            let live = dir.path().join("live.db");
            drop(Store::open(&live).unwrap());
            let missing = dir.path().join("no-such-inbox");
            let err = publish_snapshot(&live, &missing).unwrap_err();
            assert!(err.to_string().contains("does not exist"), "{err}");
            assert!(!missing.exists());

            let file = dir.path().join("not-a-dir");
            fs::write(&file, b"x").unwrap();
            let err = publish_snapshot(&live, &file).unwrap_err();
            assert!(err.to_string().contains("not a directory"), "{err}");
            assert_eq!(fs::read(&file).unwrap(), b"x");
        }

        #[test]
        fn export_stays_on_the_opened_directory_fd() {
            let dir = tempfile::TempDir::new().unwrap();
            let live = dir.path().join("live.db");
            let store = Store::open(&live).unwrap();
            let point = insert_point(&store, 4.0);
            let inbox = dir.path().join("inbox");
            fs::create_dir(&inbox).unwrap();
            let opened = open_inbox(&inbox).unwrap();
            let saved = dir.path().join("inbox-real");
            fs::rename(&inbox, &saved).unwrap();
            let victim = dir.path().join("victim");
            fs::create_dir(&victim).unwrap();
            fs::write(victim.join("data.db"), b"VICTIM").unwrap();
            fs::write(victim.join("data.db-wal"), b"VICTIM-WAL").unwrap();
            std::os::unix::fs::symlink(&victim, &inbox).unwrap();

            publish_into(&opened, &live).unwrap();

            let published = saved.join("data.db");
            assert!(published.is_file());
            assert_eq!(header_pair(&published), (1, 1));
            assert_eq!(mode_of(&published), 0o640);
            let conn = open_immutable(&published);
            let ids = metric_ids(&conn);
            drop(conn);
            assert!(ids.contains(&point.id.to_string()), "{ids:?}");
            assert_eq!(fs::read(victim.join("data.db")).unwrap(), b"VICTIM");
            assert_eq!(fs::read(victim.join("data.db-wal")).unwrap(), b"VICTIM-WAL");
            assert!(dot_tmps(&saved).is_empty(), "{:?}", dot_tmps(&saved));
            assert!(dot_tmps(&victim).is_empty(), "{:?}", dot_tmps(&victim));
        }

        #[test]
        fn busy_writer_cannot_outrun_the_export_deadline() {
            let dir = tempfile::TempDir::new().unwrap();
            let live = dir.path().join("live.db");
            drop(Store::open(&live).unwrap());

            let commits = Arc::new(AtomicU64::new(0));
            let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let commits_writer = Arc::clone(&commits);
            let stop_writer = Arc::clone(&stop);
            let live_writer = live.clone();
            let writer = std::thread::spawn(move || {
                let conn = Connection::open(&live_writer).unwrap();
                conn.busy_timeout(Duration::from_millis(200)).unwrap();
                let mut i = 0i64;
                while !stop_writer.load(Ordering::Relaxed) {
                    if conn
                        .execute_batch(&format!(
                            "BEGIN;
                             INSERT INTO metrics
                                (id, kind, category, recorded_at, day, value, unit, source)
                             VALUES ('w{i}a', 'steps', 'activity', '2026-08-01T00:00:00Z',
                                     '2026-08-01', {i}.0, 'count', 'writer');
                             INSERT INTO metrics
                                (id, kind, category, recorded_at, day, value, unit, source)
                             VALUES ('w{i}b', 'steps', 'activity', '2026-08-01T00:00:01Z',
                                     '2026-08-01', {i}.0, 'count', 'writer');
                             COMMIT;"
                        ))
                        .is_ok()
                    {
                        commits_writer.fetch_add(1, Ordering::SeqCst);
                        i += 1;
                    }
                }
            });

            let wait_started = Instant::now();
            while commits.load(Ordering::SeqCst) < 5 {
                assert!(
                    wait_started.elapsed() < Duration::from_secs(5),
                    "writer did not commit"
                );
                std::thread::sleep(Duration::from_millis(1));
            }

            let inbox = dir.path().join("inbox");
            fs::create_dir(&inbox).unwrap();
            let started = Instant::now();
            let result = publish_snapshot(&live, &inbox);
            let elapsed = started.elapsed();
            stop.store(true, Ordering::Relaxed);
            writer.join().unwrap();

            assert!(
                elapsed < EXPORT_DEADLINE,
                "busy writer made export run for {elapsed:?}"
            );
            let published = result.expect("busy writer should not stall VACUUM INTO");
            assert_eq!(published, inbox.join("data.db"));
            assert_eq!(header_pair(&published), (1, 1));
            assert_eq!(mode_of(&published), 0o640);
            assert!(dot_tmps(&inbox).is_empty(), "{:?}", dot_tmps(&inbox));
            let conn = open_immutable(&published);
            let ids = metric_ids(&conn);
            drop(conn);
            let mut batches = std::collections::BTreeMap::<i64, usize>::new();
            for id in &ids {
                let Some(rest) = id.strip_prefix('w') else {
                    continue;
                };
                let (n, which) = rest.split_at(rest.len() - 1);
                assert!(which == "a" || which == "b", "{id}");
                *batches.entry(n.parse().unwrap()).or_insert(0) += 1;
            }
            assert!(!batches.is_empty(), "snapshot missed the writer's commits");
            for (batch, count) in &batches {
                assert_eq!(*count, 2, "torn batch {batch}");
            }
            assert_eq!(header_pair(&live), (2, 2));
        }

        #[test]
        fn concurrent_writer_during_export_yields_a_consistent_snapshot() {
            let dir = tempfile::TempDir::new().unwrap();
            let live = dir.path().join("live.db");
            drop(Store::open(&live).unwrap());

            let commits = Arc::new(AtomicU64::new(0));
            let commits_writer = Arc::clone(&commits);
            let live_writer = live.clone();
            let writer = std::thread::spawn(move || {
                let conn = Connection::open(&live_writer).unwrap();
                conn.busy_timeout(Duration::from_secs(5)).unwrap();
                for i in 0..30 {
                    conn.execute_batch(&format!(
                        "BEGIN;
                         INSERT INTO metrics
                            (id, kind, category, recorded_at, day, value, unit, source)
                         VALUES ('b{i}a', 'steps', 'activity', '2026-08-01T00:00:00Z',
                                 '2026-08-01', {i}.0, 'count', 'writer');
                         INSERT INTO metrics
                            (id, kind, category, recorded_at, day, value, unit, source)
                         VALUES ('b{i}b', 'steps', 'activity', '2026-08-01T00:00:01Z',
                                 '2026-08-01', {i}.0, 'count', 'writer');
                         COMMIT;"
                    ))
                    .unwrap();
                    commits_writer.fetch_add(1, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(2));
                }
            });

            while commits.load(Ordering::SeqCst) == 0 {
                std::thread::sleep(Duration::from_millis(1));
            }
            let inbox = dir.path().join("inbox");
            fs::create_dir(&inbox).unwrap();
            publish_snapshot(&live, &inbox).unwrap();
            writer.join().unwrap();

            let snap = open_immutable(&inbox.join("data.db"));
            let ids = metric_ids(&snap);
            drop(snap);
            let mut batches = std::collections::BTreeMap::<i64, usize>::new();
            for id in &ids {
                let rest = id.strip_prefix('b').expect(id);
                let (n, which) = rest.split_at(rest.len() - 1);
                assert!(which == "a" || which == "b", "{id}");
                *batches.entry(n.parse().unwrap()).or_insert(0) += 1;
            }
            assert!(!batches.is_empty(), "snapshot missed every committed row");
            let keys: Vec<i64> = batches.keys().copied().collect();
            assert_eq!(
                keys,
                (0..keys.len() as i64).collect::<Vec<_>>(),
                "snapshot skipped a committed batch: {keys:?}"
            );
            for (batch, count) in &batches {
                assert_eq!(*count, 2, "torn batch {batch}");
            }
            assert_eq!(header_pair(&inbox.join("data.db")), (1, 1));
            assert!(dot_tmps(&inbox).is_empty());

            let live_conn = Connection::open(&live).unwrap();
            let live_ids = metric_ids(&live_conn);
            for id in &ids {
                assert!(live_ids.contains(id), "snapshot row {id} was not committed");
            }
            assert_eq!(header_pair(&live), (2, 2));
        }
    }
}
