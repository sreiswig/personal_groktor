//! Black-box tests for the `personal_groktor` binary.
//!
//! Each test spawns `CARGO_BIN_EXE_personal_groktor` with a cleared
//! environment and a temp database. Nothing is written into the repo.
//! Success paths never pass `--llm`. The LLM tests only omit configuration,
//! which returns before an HTTP client is built.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

const BIN: &str = env!("CARGO_BIN_EXE_personal_groktor");

struct Sandbox {
    dir: TempDir,
}

impl Sandbox {
    fn new() -> Self {
        Self {
            dir: TempDir::new().expect("temp dir"),
        }
    }

    fn db(&self) -> PathBuf {
        self.dir.path().join("data.db")
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    fn write(&self, name: &str, contents: impl AsRef<[u8]>) -> PathBuf {
        let path = self.path(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("fixture dir");
        }
        fs::write(&path, contents).expect("write fixture");
        path
    }

    fn command(&self) -> Command {
        let mut cmd = Command::new(BIN);
        cmd.env_clear()
            .env("HOME", self.dir.path())
            .env("XDG_DATA_HOME", self.dir.path().join("xdg-data"))
            .env("XDG_CONFIG_HOME", self.dir.path().join("xdg-config"))
            .env("XDG_CACHE_HOME", self.dir.path().join("xdg-cache"))
            .env("RUST_LOG", "info")
            .env("NO_COLOR", "1")
            .env("CLICOLOR_FORCE", "0")
            .current_dir(self.dir.path());
        cmd
    }

    /// Run the binary. `db` sets `--db` so the default data dir is never used.
    fn run(&self, db: Option<&Path>, args: &[&str]) -> Captured {
        let mut cmd = self.command();
        if let Some(db) = db {
            cmd.arg("--db").arg(db);
        }
        cmd.args(args);
        Captured::from(cmd.output().expect("spawn personal_groktor"))
    }

    fn groktor(&self, args: &[&str]) -> Captured {
        self.run(Some(&self.db()), args)
    }
}

struct Captured {
    code: i32,
    stdout: String,
    stderr: String,
}

impl Captured {
    fn from(output: Output) -> Self {
        Self {
            code: output.status.code().expect("process returned a status"),
            stdout: utf8("stdout", output.stdout),
            stderr: utf8("stderr", output.stderr),
        }
    }

    fn expect_ok(&self) -> &str {
        assert_eq!(
            self.code, 0,
            "expected exit 0\nstdout:\n{}\nstderr:\n{}",
            self.stdout, self.stderr
        );
        assert!(
            self.stderr.is_empty(),
            "expected empty stderr\nstdout:\n{}\nstderr:\n{}",
            self.stdout,
            self.stderr
        );
        &self.stdout
    }

    fn expect_err(&self, code: i32) -> &str {
        assert_eq!(
            self.code, code,
            "expected exit {code}\nstdout:\n{}\nstderr:\n{}",
            self.stdout, self.stderr
        );
        assert!(
            !self.stderr.trim().is_empty(),
            "expected readable stderr\nstdout:\n{}",
            self.stdout
        );
        assert!(
            self.stderr.contains("error:"),
            "stderr should identify the error:\n{}",
            self.stderr
        );
        assert!(
            !self.stderr.to_ascii_lowercase().contains("panic"),
            "stderr looks like a panic:\n{}",
            self.stderr
        );
        &self.stderr
    }
}

fn utf8(label: &str, bytes: Vec<u8>) -> String {
    String::from_utf8(bytes).unwrap_or_else(|err| panic!("{label} was not utf-8: {err}"))
}

fn example(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join(name)
}

fn arg(path: &Path) -> &str {
    path.to_str().expect("temp path is utf-8")
}

fn ingest_examples(sandbox: &Sandbox) {
    let csv = example("sample_metrics.csv");
    let loaded = sandbox.groktor(&["ingest", arg(&csv)]);
    let stdout = loaded.expect_ok();
    assert!(stdout.contains("count=40"), "{stdout}");
    assert!(
        stdout.contains("Ingested 40 row(s)") && stdout.contains("40 new metric(s) stored"),
        "{stdout}"
    );
    assert!(stdout.contains("0 parse error(s)"), "{stdout}");

    let json = example("sample_metrics.json");
    let loaded = sandbox.groktor(&["ingest", arg(&json)]);
    let stdout = loaded.expect_ok();
    assert!(stdout.contains("count=3"), "{stdout}");
    assert!(
        stdout.contains("Ingested 3 row(s)") && stdout.contains("3 new metric(s) stored"),
        "{stdout}"
    );
}

#[test]
fn help_and_version_succeed_without_creating_a_database() {
    let sandbox = Sandbox::new();
    let version = env!("CARGO_PKG_VERSION");

    let help = sandbox.run(None, &["--help"]);
    let stdout = help.expect_ok();
    assert!(
        stdout.contains("Turn personal health exports into clear, actionable wellbeing insight")
    );
    for name in [
        "ingest", "analyze", "brief", "digest", "note", "lab", "status",
    ] {
        assert!(stdout.contains(name), "help missing {name}:\n{stdout}");
    }

    let short = sandbox.run(None, &["-h"]);
    assert!(short.expect_ok().contains("Usage:"));

    let version_out = sandbox.run(None, &["--version"]);
    assert_eq!(
        version_out.expect_ok(),
        format!("personal_groktor {version}\n")
    );
    let short_version = sandbox.run(None, &["-V"]);
    assert_eq!(
        short_version.expect_ok(),
        format!("personal_groktor {version}\n")
    );

    let via_help = sandbox.run(None, &["help"]);
    assert!(via_help.expect_ok().contains("Usage:"));

    for command in [
        "ingest", "analyze", "brief", "digest", "note", "lab", "status",
    ] {
        let out = sandbox.run(None, &[command, "--help"]);
        let stdout = out.expect_ok();
        assert!(
            stdout.contains("Usage:"),
            "{command} --help missing usage:\n{stdout}"
        );
    }

    let lab_help = sandbox.run(None, &["lab", "--help"]);
    let stdout = lab_help.expect_ok();
    for action in [
        "start", "pause", "resume", "complete", "abandon", "day", "list", "show", "report",
    ] {
        assert!(
            stdout.contains(action),
            "lab help missing {action}:\n{stdout}"
        );
    }

    assert!(
        !sandbox.db().exists(),
        "help/version must not create the database"
    );
    assert!(!sandbox.path("xdg-data").join("personal_groktor").exists());
}

#[test]
fn sample_ingest_analyze_brief_and_digest() {
    let sandbox = Sandbox::new();
    ingest_examples(&sandbox);

    let status = sandbox.groktor(&["status"]);
    let stdout = status.expect_ok();
    assert!(stdout.contains("Metrics:   43"), "{stdout}");
    assert!(
        stdout.contains("Range:     2026-07-20 → 2026-07-29"),
        "{stdout}"
    );
    assert!(stdout.contains("Findings:  0"), "{stdout}");
    assert!(stdout.contains(arg(&sandbox.db())), "{stdout}");

    let analyzed = sandbox.groktor(&["analyze"]);
    let stdout = analyzed.expect_ok();
    assert!(
        stdout.contains("Analyzed 43 metric(s) from 2026-07-20 to 2026-07-29 → 18 finding(s)"),
        "{stdout}"
    );
    for title in [
        "HRV drop",
        "Multi-day short sleep",
        "Activity drop-off",
        "Elevated RHR after short sleep",
    ] {
        assert!(stdout.contains(title), "analyze missing {title}:\n{stdout}");
    }

    let ranged = sandbox.groktor(&["analyze", "--from", "2026-07-27", "--to", "2026-07-29"]);
    let stdout = ranged.expect_ok();
    assert!(stdout.contains("from 2026-07-27 to 2026-07-29"), "{stdout}");
    assert!(stdout.contains("finding(s)"), "{stdout}");

    let brief = sandbox.groktor(&["brief", "--day", "2026-07-29"]);
    let stdout = brief.expect_ok();
    assert!(stdout.contains("# Brief — 2026-07-29"), "{stdout}");
    assert!(stdout.contains("HRV drop"), "{stdout}");
    assert!(stdout.contains("not medical advice"), "{stdout}");
    assert!(
        stdout.contains("Not a medical device. Not diagnosis or treatment advice."),
        "{stdout}"
    );
    assert!(!stdout.contains("## Narrative"), "{stdout}");

    let again = sandbox.groktor(&["brief", "--day", "2026-07-29"]);
    let cached = again.expect_ok();
    assert!(cached.contains("# Brief — 2026-07-29"), "{cached}");
    assert!(!cached.contains("## Narrative"), "{cached}");

    let week_path = sandbox.path("out/week.md");
    let week = sandbox.groktor(&[
        "brief",
        "--week",
        "--day",
        "2026-07-29",
        "--out",
        arg(&week_path),
    ]);
    let stdout = week.expect_ok();
    assert!(stdout.contains("Wrote brief to"), "{stdout}");
    assert!(
        stdout.contains("# Brief — week 2026-07-23 → 2026-07-29"),
        "{stdout}"
    );
    let week_file = fs::read_to_string(&week_path).expect("week brief file");
    assert!(week_file.contains("# Brief — week 2026-07-23 → 2026-07-29"));
    assert!(week_file.contains("Not a medical device. Not diagnosis or treatment advice."));
    assert!(!week_file.contains("## Narrative"));

    let digest_path = sandbox.path("out/digest.md");
    let digest = sandbox.groktor(&["digest", "--day", "2026-07-28", "--out", arg(&digest_path)]);
    let stdout = digest.expect_ok();
    assert!(stdout.contains("# Brief — 2026-07-28"), "{stdout}");
    assert!(stdout.contains("Wrote brief to"), "{stdout}");
    assert!(!stdout.contains("## Narrative"), "{stdout}");
    let digest_file = fs::read_to_string(&digest_path).expect("digest file");
    assert!(digest_file.contains("# Brief — 2026-07-28"));
    assert!(digest_file.contains("not medical advice"));
}

#[test]
fn note_and_lab_commands_on_sample_data() {
    let sandbox = Sandbox::new();
    ingest_examples(&sandbox);

    let noted = sandbox.groktor(&[
        "note",
        "--day",
        "2026-07-28",
        "--tag",
        "alcohol",
        "-m",
        "dinner out",
        "--mood",
        "3",
        "--energy",
        "4",
    ]);
    let stdout = noted.expect_ok();
    assert!(
        stdout.contains("Logged note on 2026-07-28: tags=[alcohol]"),
        "{stdout}"
    );

    let listed = sandbox.groktor(&["note", "list", "--from", "2026-07-01", "--to", "2026-07-31"]);
    let stdout = listed.expect_ok();
    assert!(
        stdout.contains("2026-07-28  [alcohol]  dinner out"),
        "{stdout}"
    );

    let tagged = sandbox.groktor(&[
        "note",
        "list",
        "--tag",
        "alcohol",
        "--from",
        "2026-07-01",
        "--to",
        "2026-07-31",
    ]);
    assert!(tagged.expect_ok().contains("dinner out"));

    let started = sandbox.groktor(&[
        "lab",
        "start",
        "early_bed",
        "--title",
        "Earlier bedtime",
        "--hypothesis",
        "Bed by 22:30 improves next-day HRV",
        "--outcome",
        "hrv:up",
        "--primary",
        "hrv",
        "--min-days",
        "7",
        "--started-on",
        "2026-07-20",
    ]);
    let stdout = started.expect_ok();
    assert!(
        stdout.contains("Started experiment `early_bed` (status=active, min_days=7)"),
        "{stdout}"
    );
    assert!(
        stdout.contains("outcome: heart_rate_variability_ms up primary"),
        "{stdout}"
    );

    let control = sandbox.groktor(&[
        "lab",
        "day",
        "early_bed",
        "--from",
        "2026-07-20",
        "--to",
        "2026-07-24",
        "--arm",
        "control",
    ]);
    assert!(control
        .expect_ok()
        .contains("Assigned control to 5 day(s) on `early_bed` (2026-07-20 → 2026-07-24)"));

    let intervention = sandbox.groktor(&[
        "lab",
        "day",
        "early_bed",
        "--from",
        "2026-07-25",
        "--to",
        "2026-07-29",
        "--arm",
        "intervention",
    ]);
    assert!(intervention
        .expect_ok()
        .contains("Assigned intervention to 5 day(s) on `early_bed`"));

    let listed = sandbox.groktor(&["lab", "list"]);
    let stdout = listed.expect_ok();
    assert!(stdout.contains("early_bed"), "{stdout}");
    assert!(stdout.contains("Earlier bedtime"), "{stdout}");
    assert!(stdout.contains("active"), "{stdout}");

    let show = sandbox.groktor(&["lab", "show", "early_bed"]);
    let stdout = show.expect_ok();
    assert!(
        stdout.contains("hypothesis:  Bed by 22:30 improves next-day HRV"),
        "{stdout}"
    );
    assert!(
        stdout.contains("arm days:    intervention=5 control=5 exclude=0"),
        "{stdout}"
    );

    let report_path = sandbox.path("out/lab.md");
    let report = sandbox.groktor(&["lab", "report", "early_bed", "--out", arg(&report_path)]);
    let stdout = report.expect_ok();
    assert!(stdout.contains("Wrote lab report to"), "{stdout}");
    assert!(stdout.contains("# Lab — early_bed"), "{stdout}");
    assert!(stdout.contains("43.60"), "{stdout}");
    assert!(stdout.contains("Descriptive N=1 only"), "{stdout}");
    assert!(
        stdout.contains("Not a medical device. Not diagnosis or treatment advice."),
        "{stdout}"
    );
    let report_file = fs::read_to_string(&report_path).expect("lab report file");
    assert!(report_file.contains("# Lab — early_bed"));
    assert!(report_file.contains("not medical advice"));

    let one_day = sandbox.groktor(&[
        "lab",
        "day",
        "early_bed",
        "--day",
        "2026-07-19",
        "--arm",
        "exclude",
    ]);
    assert!(one_day
        .expect_ok()
        .contains("Assigned exclude on 2026-07-19 for `early_bed`"));

    assert!(sandbox
        .groktor(&["lab", "pause", "early_bed"])
        .expect_ok()
        .contains("Paused `early_bed`"));
    assert!(sandbox
        .groktor(&["lab", "resume", "early_bed"])
        .expect_ok()
        .contains("Resumed `early_bed`"));
    assert!(sandbox
        .groktor(&["lab", "complete", "early_bed"])
        .expect_ok()
        .contains("Completed `early_bed`"));

    let status = sandbox.groktor(&["status"]);
    let stdout = status.expect_ok();
    assert!(stdout.contains("Notes:     1"), "{stdout}");
    assert!(
        stdout.contains("Labs:      0 active, 1 completed"),
        "{stdout}"
    );
    assert!(stdout.contains("Metrics:   43"), "{stdout}");

    assert!(sandbox
        .groktor(&["lab", "abandon", "early_bed", "--reason", "done testing"])
        .expect_ok()
        .contains("Abandoned `early_bed`"));
    let status = sandbox.groktor(&["status"]);
    assert!(status
        .expect_ok()
        .contains("Labs:      0 active, 0 completed"));
}

#[test]
fn db_flag_after_subcommand_and_groktor_db_env() {
    let sandbox = Sandbox::new();
    let flagged = sandbox.path("flagged.db");
    let csv = example("sample_metrics.csv");
    let ingested = sandbox.run(None, &["ingest", "--db", arg(&flagged), arg(&csv)]);
    let stdout = ingested.expect_ok();
    assert!(stdout.contains("40 new metric(s) stored"), "{stdout}");
    assert!(flagged.exists());

    let status = sandbox.run(None, &["status", "--db", arg(&flagged)]);
    assert!(status.expect_ok().contains("Metrics:   40"));

    let via_env = sandbox.path("env.db");
    let mut cmd = sandbox.command();
    cmd.env("GROKTOR_DB", &via_env);
    cmd.args(["ingest", arg(&csv)]);
    let ingested = Captured::from(cmd.output().expect("spawn"));
    assert!(ingested.expect_ok().contains("40 new metric(s) stored"));

    let mut cmd = sandbox.command();
    cmd.env("GROKTOR_DB", &via_env);
    cmd.arg("status");
    let status = Captured::from(cmd.output().expect("spawn"));
    let stdout = status.expect_ok();
    assert!(stdout.contains("Metrics:   40"), "{stdout}");
    assert!(stdout.contains(arg(&via_env)), "{stdout}");
    assert!(
        !stdout.contains(arg(&flagged)),
        "GROKTOR_DB must not read the other database:\n{stdout}"
    );
}

#[test]
fn missing_file_malformed_csv_and_bad_json_fail() {
    let sandbox = Sandbox::new();

    let missing = sandbox.groktor(&["ingest", arg(&sandbox.path("missing.csv"))]);
    let stderr = missing.expect_err(1);
    assert!(stderr.contains("No such file or directory"), "{stderr}");
    assert!(missing.stdout.is_empty(), "{}", missing.stdout);

    let bad_float = sandbox.write(
        "bad-float.csv",
        "date,metric,value\n2026-08-01,steps,not-a-number\n",
    );
    let failed = sandbox.groktor(&["ingest", arg(&bad_float)]);
    let stderr = failed.expect_err(1);
    assert!(stderr.contains("CSV error:"), "{stderr}");
    assert!(stderr.contains("invalid float literal"), "{stderr}");

    let bad_record = sandbox.write(
        "bad-record.csv",
        "date,metric,value\n\"unterminated,steps,1\n",
    );
    let failed = sandbox.groktor(&["ingest", arg(&bad_record)]);
    let stderr = failed.expect_err(1);
    assert!(stderr.contains("CSV error:"), "{stderr}");

    let text = sandbox.write("notes.txt", "not a health export\n");
    let failed = sandbox.groktor(&["ingest", arg(&text)]);
    let stderr = failed.expect_err(1);
    assert!(
        stderr.contains("Unsupported format") && stderr.contains("expected .csv or .json"),
        "{stderr}"
    );
    assert!(stderr.contains(arg(&text)), "{stderr}");

    for (name, body) in [
        ("truncated.json", "{"),
        ("object.json", "{}"),
        ("string.json", "\"hello\""),
    ] {
        let path = sandbox.write(name, body);
        let failed = sandbox.groktor(&["ingest", arg(&path)]);
        let stderr = failed.expect_err(1);
        assert!(
            stderr.contains("Parse error: JSON must be an array"),
            "{name}: {stderr}"
        );
    }

    let status = sandbox.groktor(&["status"]);
    assert!(
        status.expect_ok().contains("Metrics:   0"),
        "failed ingests must not store metrics"
    );
}

#[test]
fn non_finite_csv_values_are_rejected_and_reported() {
    // CSV `NaN` / `inf` spellings parse as f64. Ingest keeps the finite rows,
    // logs one warning per rejected row, and exits 0. The warning text is the
    // readable rejection; tracing writes it to stdout.
    let sandbox = Sandbox::new();
    let csv = sandbox.write(
        "non-finite.csv",
        "\
date,metric,value,unit,source
2026-08-01,steps,8000,count,fitbit
2026-08-01,rhr,NaN,bpm,oura
2026-08-02,hrv,inf,ms,oura
2026-08-02,sleep_hours,-inf,hours,oura
2026-08-01,rhr,+inf,bpm,oura
2026-08-01,hrv,Infinity,ms,oura
2026-08-01,sleep_hours,-Infinity,hours,oura
2026-08-01,weight,+Infinity,kg,scale
2026-08-02,spo2,1e309,%,pulse
2026-08-02,body_fat,-1e309,%,scale
2026-08-03,steps,100,count,fitbit
",
    );

    let ingested = sandbox.groktor(&["ingest", arg(&csv)]);
    let stdout = ingested.expect_ok();
    assert!(stdout.contains("non-finite value NaN"), "{stdout}");
    assert!(stdout.contains("non-finite value inf"), "{stdout}");
    assert!(stdout.contains("non-finite value -inf"), "{stdout}");
    let warnings = stdout
        .lines()
        .filter(|line| line.contains("non-finite value"))
        .count();
    assert_eq!(warnings, 9, "{stdout}");
    assert!(
        stdout.contains("11 row(s)")
            && stdout.contains("2 new metric(s) stored (9 parse error(s))"),
        "{stdout}"
    );
    for line in stdout.lines().filter(|line| line.contains("non-finite")) {
        assert!(
            !line.chars().any(|c| c.is_control()),
            "warning must stay one readable line: {line:?}"
        );
    }

    let status = sandbox.groktor(&["status"]);
    let stdout = status.expect_ok();
    assert!(stdout.contains("Metrics:   2"), "{stdout}");
    assert!(
        stdout.contains("Range:     2026-08-01 → 2026-08-03"),
        "{stdout}"
    );

    let empty = Sandbox::new();
    let only_bad = empty.write(
        "all-non-finite.csv",
        "date,metric,value\n2026-08-01,rhr,NaN\n2026-08-01,hrv,inf\n2026-08-02,sleep_hours,-inf\n",
    );
    let ingested = empty.groktor(&["ingest", arg(&only_bad)]);
    let stdout = ingested.expect_ok();
    assert!(
        stdout.contains("0 new metric(s) stored (3 parse error(s))"),
        "{stdout}"
    );
    assert_eq!(
        stdout
            .lines()
            .filter(|line| line.contains("non-finite value"))
            .count(),
        3,
        "{stdout}"
    );
    assert!(empty
        .groktor(&["status"])
        .expect_ok()
        .contains("Metrics:   0"));
    assert!(empty
        .groktor(&["status"])
        .expect_ok()
        .contains("Range:     (empty)"));
}

#[test]
fn non_finite_json_documents_fail() {
    let sandbox = Sandbox::new();
    let bodies = [
        r#"[{"date":"2026-08-01","metric":"rhr","value":NaN}]"#,
        r#"[{"date":"2026-08-01","metric":"rhr","value":Infinity}]"#,
        r#"[{"date":"2026-08-01","metric":"rhr","value":-Infinity}]"#,
        r#"{"metrics":[{"date":"2026-08-01","metric":"hrv","value":NaN}]}"#,
        r#"[{"date":"2026-08-01","metric":"steps","value":1e309}]"#,
        r#"[{"date":"2026-08-01","metric":"steps","value":-1e309}]"#,
    ];
    for (i, body) in bodies.iter().enumerate() {
        let path = sandbox.write(&format!("nan-{i}.json"), *body);
        let failed = sandbox.groktor(&["ingest", arg(&path)]);
        let stderr = failed.expect_err(1);
        assert!(
            stderr.contains("Parse error: JSON must be an array"),
            "{body} -> {stderr}"
        );
        assert!(failed.stdout.is_empty(), "{}", failed.stdout);
    }

    let status = sandbox.groktor(&["status"]);
    assert!(status.expect_ok().contains("Metrics:   0"));
}

#[test]
fn control_characters_in_ingest_errors_are_escaped() {
    let sandbox = Sandbox::new();
    let csv = sandbox.write(
        "controls.csv",
        b"date,metric,value\n\
          2026-08-01,\"rhr\nforged\x1b[2J\",NaN\n\
          \"2026-08-01\nINFO forged\",steps,1\n\
          2026-08-01,steps,100\n",
    );

    let ingested = sandbox.groktor(&["ingest", arg(&csv)]);
    let stdout = ingested.expect_ok();
    assert!(
        stdout.contains(r"non-finite value NaN for metric 'rhr\nforged\u{1b}[2J'"),
        "{stdout}"
    );
    assert!(
        stdout.contains(r"unrecognized date: 2026-08-01\nINFO forged"),
        "{stdout}"
    );
    assert!(
        !stdout.contains('\u{1b}'),
        "raw ESC must not reach the log: {stdout:?}"
    );
    for line in stdout.lines() {
        assert!(
            !line.chars().any(|c| c.is_control()),
            "log line contains a raw control: {line:?}"
        );
    }
    assert!(
        stdout.contains("1 new metric(s) stored (2 parse error(s))"),
        "{stdout}"
    );

    let status = sandbox.groktor(&["status"]);
    assert!(status.expect_ok().contains("Metrics:   1"));
}

#[test]
fn empty_database_header_only_csv_and_usage_errors() {
    let sandbox = Sandbox::new();

    let status = sandbox.groktor(&["status"]);
    let stdout = status.expect_ok();
    assert!(stdout.contains("Metrics:   0"), "{stdout}");
    assert!(stdout.contains("Range:     (empty)"), "{stdout}");
    assert!(stdout.contains("Notes:     0"), "{stdout}");
    assert!(
        stdout.contains("Labs:      0 active, 0 completed"),
        "{stdout}"
    );

    let analyzed = sandbox.groktor(&["analyze"]);
    assert!(analyzed
        .expect_ok()
        .contains("No metrics in database. Run `ingest` first."));

    let brief = sandbox.groktor(&["brief"]);
    let stderr = brief.expect_err(1);
    assert!(
        stderr.contains("No metrics in database. Run `ingest` first."),
        "{stderr}"
    );

    let header = sandbox.write("header.csv", "date,metric,value\n");
    let empty = sandbox.groktor(&["ingest", arg(&header)]);
    assert!(empty.expect_ok().contains("No rows found"));

    let empty_json = sandbox.write("empty.json", "[]");
    assert!(sandbox
        .groktor(&["ingest", arg(&empty_json)])
        .expect_ok()
        .contains("No rows found"));

    let listed = sandbox.groktor(&["note", "list"]);
    assert!(listed.expect_ok().contains("No annotations in range."));

    let bare = sandbox.groktor(&["note"]);
    let stderr = bare.expect_err(1);
    assert!(stderr.contains("usage: note"), "{stderr}");

    let mood = sandbox.groktor(&[
        "note",
        "--tag",
        "sleep",
        "--mood",
        "9",
        "--day",
        "2026-07-20",
    ]);
    let stderr = mood.expect_err(1);
    assert!(stderr.contains("mood must be 1–5"), "{stderr}");

    let arm = sandbox.groktor(&[
        "note",
        "--arm",
        "control",
        "-m",
        "hi",
        "--day",
        "2026-07-20",
    ]);
    let stderr = arm.expect_err(1);
    assert!(stderr.contains("--arm requires --experiment"), "{stderr}");

    let unknown = sandbox.run(None, &["normalize"]);
    let stderr = unknown.expect_err(2);
    assert!(stderr.contains("unrecognized subcommand"), "{stderr}");

    let missing_path = sandbox.groktor(&["ingest"]);
    let stderr = missing_path.expect_err(2);
    assert!(stderr.contains("required arguments"), "{stderr}");

    let missing_outcome = sandbox.groktor(&[
        "lab",
        "start",
        "early_bed",
        "--title",
        "Earlier bedtime",
        "--hypothesis",
        "Bed by 22:30 improves next-day HRV",
    ]);
    let stderr = missing_outcome.expect_err(2);
    assert!(stderr.contains("--outcome"), "{stderr}");

    let started = sandbox.groktor(&[
        "lab",
        "start",
        "early_bed",
        "--title",
        "Earlier bedtime",
        "--hypothesis",
        "Bed by 22:30 improves next-day HRV",
        "--outcome",
        "hrv:up",
    ]);
    started.expect_ok();

    let bad_arm = sandbox.groktor(&[
        "lab",
        "day",
        "early_bed",
        "--day",
        "2026-07-20",
        "--arm",
        "sideways",
    ]);
    let stderr = bad_arm.expect_err(1);
    assert!(stderr.contains("unknown arm `sideways`"), "{stderr}");

    let missing = sandbox.groktor(&["lab", "show", "nope"]);
    let stderr = missing.expect_err(1);
    assert!(stderr.contains("unknown experiment slug: nope"), "{stderr}");

    let bad_day = sandbox.groktor(&["analyze", "--from", "not-a-date"]);
    let stderr = bad_day.expect_err(2);
    assert!(
        stderr.contains("not-a-date") || stderr.contains("--from"),
        "{stderr}"
    );
}

#[test]
fn llm_switches_fail_closed_without_calling_a_backend() {
    let sandbox = Sandbox::new();
    let csv = example("sample_metrics.csv");
    sandbox.groktor(&["ingest", arg(&csv)]).expect_ok();

    let grok = sandbox.groktor(&["brief", "--day", "2026-07-29", "--llm"]);
    let stderr = grok.expect_err(1);
    assert!(stderr.contains("XAI_API_KEY is not set"), "{stderr}");
    assert!(grok.stdout.is_empty(), "{}", grok.stdout);
    assert!(!stderr.contains("## Narrative"), "{stderr}");

    let digest = sandbox.groktor(&["digest", "--day", "2026-07-29", "--llm"]);
    let stderr = digest.expect_err(1);
    assert!(stderr.contains("XAI_API_KEY is not set"), "{stderr}");
    assert!(digest.stdout.is_empty(), "{}", digest.stdout);

    let local = sandbox.groktor(&[
        "brief",
        "--day",
        "2026-07-29",
        "--llm",
        "--llm-backend",
        "local",
    ]);
    let stderr = local.expect_err(1);
    assert!(
        stderr.contains("GROKTOR_LLM_BASE") && stderr.contains("will not phone home"),
        "{stderr}"
    );
    assert!(local.stdout.is_empty(), "{}", local.stdout);

    let mut cmd = sandbox.command();
    cmd.arg("--db").arg(sandbox.db());
    cmd.env("GROKTOR_LLM_BASE", "http://127.0.0.1:9/v1");
    cmd.args([
        "brief",
        "--day",
        "2026-07-29",
        "--llm",
        "--llm-backend",
        "local",
    ]);
    let missing_model = Captured::from(cmd.output().expect("spawn"));
    let stderr = missing_model.expect_err(1);
    assert!(
        stderr.contains("GROKTOR_LLM_MODEL") || stderr.contains("LLM_MODEL"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("connection") && !stderr.contains("timed out"),
        "model is required before any request:\n{stderr}"
    );
}
