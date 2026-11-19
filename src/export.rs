//! Rollback-mode snapshot of the live health database for the warehouse loader.
//!
//! The live database stays in `journal_mode=WAL`. The snapshot is a separate
//! file, `data.db`, in a caller-chosen inbox directory. `VACUUM INTO` cannot
//! write a path that already exists, so it cannot fill a file created with
//! `O_EXCL`. The copy uses the SQLite online backup API instead, then
//! `journal_mode=DELETE` so the published file is rollback mode.

use std::path::{Path, PathBuf};

#[cfg(not(target_os = "linux"))]
use crate::error::GroktorError;
use crate::error::Result;

/// Publish a consistent snapshot of `live_db` as `inbox/data.db`.
///
/// `inbox` must already exist and must not be a symlink. `inbox/data.db`,
/// when it already exists, must be a regular file. The previous `data.db` is
/// left in place if anything fails before `rename`.
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
    use std::fs::{self, File, OpenOptions};
    use std::io::{self, Read};
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    use rusqlite::backup::{Backup, StepResult};
    use rusqlite::{Connection, OpenFlags};
    use uuid::Uuid;

    use crate::error::{GroktorError, Result};

    /// Group-readable, owner-writable. A setgid inbox directory (the warehouse
    /// inbox is `2770`) gives the file the inbox group, so the loader can read
    /// it. World has no bits. `fchmod` applies this even when umask is wider.
    const SNAPSHOT_MODE: u32 = 0o640;
    /// Linux `O_NOFOLLOW`.
    const O_NOFOLLOW: i32 = 0o400000;
    /// Linux `O_DIRECTORY`.
    const O_DIRECTORY: i32 = 0o200000;
    const SIDECAR_SUFFIXES: [&str; 3] = ["-wal", "-shm", "-journal"];
    /// Bound for a source that keeps changing. A quiet health database finishes
    /// well inside this. On timeout the temp file is removed and the previous
    /// `data.db` stays.
    const BACKUP_DEADLINE: Duration = Duration::from_secs(60);

    struct OpenedInbox {
        path: PathBuf,
        dir: File,
    }

    struct TempGuard {
        path: PathBuf,
        armed: bool,
    }

    impl TempGuard {
        fn arm(path: &Path) -> Self {
            Self {
                path: path.to_path_buf(),
                armed: true,
            }
        }

        fn commit(&mut self) {
            self.armed = false;
        }
    }

    impl Drop for TempGuard {
        fn drop(&mut self) {
            if !self.armed {
                return;
            }
            let _ = fs::remove_file(&self.path);
            for suffix in SIDECAR_SUFFIXES {
                let _ = fs::remove_file(sidecar_path(&self.path, suffix));
            }
        }
    }

    pub(super) fn publish(live_db: &Path, inbox: &Path) -> Result<PathBuf> {
        let inbox = open_inbox(inbox)?;
        let dest = inbox.path.join("data.db");
        refuse_live_target(&dest, live_db)?;
        require_regular_target(&dest)?;

        let (tmp, created) = create_temp(&inbox.path)?;
        debug_assert_eq!(tmp.parent(), Some(inbox.path.as_path()));
        let mut guard = TempGuard::arm(&tmp);
        write_and_publish(&inbox, &dest, live_db, &tmp, created, &mut guard)?;
        Ok(dest)
    }

    fn write_and_publish(
        inbox: &OpenedInbox,
        dest: &Path,
        live_db: &Path,
        tmp: &Path,
        created: (u64, u64),
        guard: &mut TempGuard,
    ) -> Result<()> {
        copy_consistent(live_db, tmp, created)?;
        ensure_no_sidecars(tmp)?;
        ensure_rollback_header(tmp)?;
        ensure_mode(tmp)?;
        fsync_no_follow(tmp)?;
        ensure_rollback_header(tmp)?;
        refuse_live_target(dest, live_db)?;
        require_regular_target(dest)?;
        fs::rename(tmp, dest).map_err(|err| {
            GroktorError::Message(format!(
                "failed to rename snapshot onto {} (existing data.db was left unchanged): {err}",
                dest.display()
            ))
        })?;
        // The temp name is gone. Disarm before any later step so a failure
        // cannot unlink a different file that reused the name.
        guard.commit();
        remove_replaced_sidecars(dest)?;
        ensure_no_sidecars(dest)?;
        inbox.dir.sync_all().map_err(|err| {
            GroktorError::Message(format!(
                "snapshot was renamed to {} but the inbox directory could not be fsynced: {err}",
                dest.display()
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
        Ok(OpenedInbox {
            path: inbox.to_path_buf(),
            dir,
        })
    }

    fn refuse_live_target(dest: &Path, live: &Path) -> Result<()> {
        if same_database(dest, live) {
            return Err(GroktorError::Message(format!(
                "refusing to replace the live database ({}) with the snapshot",
                live.display()
            )));
        }
        Ok(())
    }

    fn same_database(dest: &Path, live: &Path) -> bool {
        if let (Ok(dest_abs), Ok(live_abs)) = (std::path::absolute(dest), std::path::absolute(live))
        {
            if dest_abs == live_abs {
                return true;
            }
        }
        match (fs::symlink_metadata(dest), fs::symlink_metadata(live)) {
            (Ok(dest_meta), Ok(live_meta)) => {
                dest_meta.dev() == live_meta.dev() && dest_meta.ino() == live_meta.ino()
            }
            _ => false,
        }
    }

    fn require_regular_target(dest: &Path) -> Result<()> {
        match fs::symlink_metadata(dest) {
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(GroktorError::Message(format!(
                "failed to inspect export target {}: {err}",
                dest.display()
            ))),
            Ok(meta) if meta.file_type().is_symlink() => Err(GroktorError::Message(format!(
                "export target is a symlink ({}); refusing to follow it",
                dest.display()
            ))),
            Ok(meta) if meta.is_file() => Ok(()),
            Ok(_) => Err(GroktorError::Message(format!(
                "export target exists and is not a regular file: {}",
                dest.display()
            ))),
        }
    }

    fn temp_file_name() -> String {
        format!(".data.db.{}.tmp", Uuid::new_v4().as_simple())
    }

    fn create_temp(inbox: &Path) -> Result<(PathBuf, (u64, u64))> {
        for _ in 0..8 {
            let path = inbox.join(temp_file_name());
            match create_exclusive(&path) {
                Ok(ids) => return Ok((path, ids)),
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(err) => {
                    return Err(GroktorError::Message(format!(
                        "failed to create snapshot temp file in {}: {err}",
                        inbox.display()
                    )));
                }
            }
        }
        Err(GroktorError::Message(
            "failed to create a unique snapshot temp file after 8 attempts".into(),
        ))
    }

    fn create_exclusive(path: &Path) -> io::Result<(u64, u64)> {
        let mut opts = OpenOptions::new();
        opts.read(true)
            .write(true)
            .create_new(true)
            .mode(SNAPSHOT_MODE)
            .custom_flags(O_NOFOLLOW);
        let file = opts.open(path)?;
        let finished = (|| -> io::Result<(u64, u64)> {
            file.set_permissions(fs::Permissions::from_mode(SNAPSHOT_MODE))?;
            let meta = file.metadata()?;
            let mode = meta.permissions().mode() & 0o777;
            if !mode_is_acceptable(mode) {
                return Err(io::Error::other(format!(
                    "temp file mode is {mode:04o}, want 0640 or tighter"
                )));
            }
            Ok((meta.dev(), meta.ino()))
        })();
        match finished {
            Ok(ids) => Ok(ids),
            Err(err) => {
                drop(file);
                let _ = fs::remove_file(path);
                Err(err)
            }
        }
    }

    fn mode_is_acceptable(mode: u32) -> bool {
        let bits = mode & 0o777;
        bits & !SNAPSHOT_MODE == 0 && bits & 0o600 == 0o600
    }

    fn copy_consistent(live_db: &Path, tmp: &Path, created: (u64, u64)) -> Result<()> {
        // Read-only so this connection cannot create the live file, change its
        // journal mode, or checkpoint it. Committed WAL frames are visible
        // while another connection (the open Store) holds the -shm.
        let src = Connection::open_with_flags(live_db, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(
            |err| {
                GroktorError::Message(format!(
                    "failed to open the live database read-only ({}): {err}",
                    live_db.display()
                ))
            },
        )?;
        src.busy_timeout(Duration::from_secs(5))?;
        src.execute_batch("PRAGMA query_only = ON;")?;

        let mut dest = Connection::open(tmp).map_err(|err| {
            GroktorError::Message(format!(
                "failed to open snapshot temp file {}: {err}",
                tmp.display()
            ))
        })?;
        confirm_same_file(tmp, created)?;
        dest.busy_timeout(Duration::from_secs(5))?;

        {
            let backup = Backup::new(&src, &mut dest)?;
            backup_to_completion(&backup)?;
        }

        let mode: String = dest.query_row("PRAGMA journal_mode = DELETE", [], |row| row.get(0))?;
        if !mode.eq_ignore_ascii_case("delete") {
            return Err(GroktorError::Message(format!(
                "snapshot journal_mode stayed {mode}; rollback mode (DELETE) is required"
            )));
        }
        let messages = integrity_messages(&dest)?;
        if messages != ["ok"] {
            return Err(GroktorError::Message(format!(
                "snapshot integrity_check failed: {}",
                messages.join("; ")
            )));
        }

        dest.close().map_err(|(_, err)| {
            GroktorError::Message(format!(
                "failed to close snapshot temp file {}: {err}",
                tmp.display()
            ))
        })?;
        src.close().map_err(|(_, err)| {
            GroktorError::Message(format!(
                "failed to close the read-only live database handle ({}): {err}",
                live_db.display()
            ))
        })?;
        Ok(())
    }

    fn backup_to_completion(backup: &Backup) -> Result<()> {
        let deadline = Instant::now() + BACKUP_DEADLINE;
        loop {
            if Instant::now() >= deadline {
                return Err(GroktorError::Message(
                    "snapshot backup did not finish within 60s; the live database stayed busy \
                     and the previous data.db was left unchanged"
                        .into(),
                ));
            }
            match backup.step(64)? {
                StepResult::Done => return Ok(()),
                StepResult::More | StepResult::Busy | StepResult::Locked => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                _ => {
                    return Err(GroktorError::Message(
                        "snapshot backup returned an unexpected step result; the previous data.db was left unchanged"
                            .into(),
                    ));
                }
            }
        }
    }

    fn confirm_same_file(path: &Path, created: (u64, u64)) -> Result<()> {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(O_NOFOLLOW)
            .open(path)
            .map_err(|err| {
                GroktorError::Message(format!(
                    "snapshot temp file {} could not be reopened without following a symlink: {err}",
                    path.display()
                ))
            })?;
        let meta = file.metadata().map_err(|err| {
            GroktorError::Message(format!(
                "failed to stat snapshot temp file {}: {err}",
                path.display()
            ))
        })?;
        if meta.file_type().is_symlink() || !meta.is_file() {
            return Err(GroktorError::Message(format!(
                "snapshot temp file {} is no longer a regular file",
                path.display()
            )));
        }
        if (meta.dev(), meta.ino()) != created {
            return Err(GroktorError::Message(format!(
                "snapshot temp file {} was replaced after it was created; refusing to follow it",
                path.display()
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

    fn ensure_no_sidecars(db: &Path) -> Result<()> {
        for suffix in SIDECAR_SUFFIXES {
            let path = sidecar_path(db, suffix);
            match fs::symlink_metadata(&path) {
                Err(err) if err.kind() == io::ErrorKind::NotFound => {}
                Err(err) => {
                    return Err(GroktorError::Message(format!(
                        "failed to inspect sidecar {}: {err}",
                        path.display()
                    )));
                }
                Ok(_) => {
                    return Err(GroktorError::Message(format!(
                        "snapshot left sidecar {}; refusing to publish a file that is not a single rollback database",
                        path.display()
                    )));
                }
            }
        }
        Ok(())
    }

    /// Drop `-wal` / `-shm` / `-journal` names left beside a previous `data.db`.
    /// `remove_file` unlinks a symlink and does not follow it.
    fn remove_replaced_sidecars(db: &Path) -> Result<()> {
        for suffix in SIDECAR_SUFFIXES {
            let path = sidecar_path(db, suffix);
            match fs::symlink_metadata(&path) {
                Err(err) if err.kind() == io::ErrorKind::NotFound => {}
                Err(err) => {
                    return Err(GroktorError::Message(format!(
                        "failed to inspect sidecar {}: {err}",
                        path.display()
                    )));
                }
                Ok(meta) if meta.file_type().is_symlink() || meta.is_file() => {
                    fs::remove_file(&path).map_err(|err| {
                        GroktorError::Message(format!(
                            "failed to remove sidecar {} without following it: {err}",
                            path.display()
                        ))
                    })?;
                }
                Ok(_) => {
                    return Err(GroktorError::Message(format!(
                        "sidecar {} is not a regular file; left it in place",
                        path.display()
                    )));
                }
            }
        }
        Ok(())
    }

    fn ensure_rollback_header(path: &Path) -> Result<()> {
        let (write_version, read_version) = header_versions(path)?;
        if write_version != 1 || read_version != 1 {
            return Err(GroktorError::Message(format!(
                "snapshot is not in rollback journal mode (header bytes 18 and 19 are {write_version} and {read_version}, expected 1 and 1)"
            )));
        }
        Ok(())
    }

    fn header_versions(path: &Path) -> Result<(u8, u8)> {
        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(O_NOFOLLOW)
            .open(path)
            .map_err(|err| {
                GroktorError::Message(format!(
                    "failed to read snapshot header {}: {err}",
                    path.display()
                ))
            })?;
        let mut hdr = [0u8; 20];
        file.read_exact(&mut hdr).map_err(|err| {
            GroktorError::Message(format!(
                "snapshot {} is too small to be a SQLite database: {err}",
                path.display()
            ))
        })?;
        if &hdr[0..16] != b"SQLite format 3\0" {
            return Err(GroktorError::Message(format!(
                "snapshot {} is not a SQLite database",
                path.display()
            )));
        }
        Ok((hdr[18], hdr[19]))
    }

    fn ensure_mode(path: &Path) -> Result<()> {
        let meta = fs::symlink_metadata(path).map_err(|err| {
            GroktorError::Message(format!("failed to stat snapshot {}: {err}", path.display()))
        })?;
        if meta.file_type().is_symlink() {
            return Err(GroktorError::Message(format!(
                "snapshot {} is a symlink; refusing to follow it",
                path.display()
            )));
        }
        let mode = meta.permissions().mode() & 0o777;
        if !mode_is_acceptable(mode) {
            return Err(GroktorError::Message(format!(
                "snapshot mode is {mode:04o}; expected 0640 or tighter with owner read and write"
            )));
        }
        Ok(())
    }

    fn fsync_no_follow(path: &Path) -> Result<()> {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(O_NOFOLLOW)
            .open(path)
            .map_err(|err| {
                GroktorError::Message(format!(
                    "failed to open {} for fsync: {err}",
                    path.display()
                ))
            })?;
        file.sync_all().map_err(|err| {
            GroktorError::Message(format!("failed to fsync {}: {err}", path.display()))
        })?;
        Ok(())
    }

    fn sidecar_path(db: &Path, suffix: &str) -> PathBuf {
        let mut name = db.as_os_str().to_os_string();
        name.push(suffix);
        PathBuf::from(name)
    }

    #[cfg(test)]
    mod tests {
        use std::collections::BTreeMap;
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
        fn exclusive_create_sets_mode_and_does_not_follow_a_symlink() {
            let dir = tempfile::TempDir::new().unwrap();
            let target = dir.path().join("target");
            fs::write(&target, b"keep").unwrap();
            let link = dir.path().join(".data.db.link.tmp");
            std::os::unix::fs::symlink(&target, &link).unwrap();
            let err = create_exclusive(&link).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
            assert_eq!(fs::read(&target).unwrap(), b"keep");
            assert!(fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink());

            let fresh = dir.path().join(".data.db.fresh.tmp");
            create_exclusive(&fresh).unwrap();
            assert_eq!(mode_of(&fresh), 0o640);
            let again = create_exclusive(&fresh).unwrap_err();
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
        fn concurrent_writer_during_export_yields_a_consistent_snapshot() {
            let dir = tempfile::TempDir::new().unwrap();
            let live = dir.path().join("live.db");
            let store = Store::open(&live).unwrap();
            drop(store);

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
            let mut batches: BTreeMap<i64, usize> = BTreeMap::new();
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
