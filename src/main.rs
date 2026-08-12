//! Personal Groktor CLI.

use std::path::PathBuf;

use chrono::{NaiveDate, Utc};
use clap::{Parser, Subcommand};
use personal_groktor::analyze;
use personal_groktor::brief::{self, BriefOptions};
use personal_groktor::ingest;
use personal_groktor::lab;
use personal_groktor::llm::GrokClient;
use personal_groktor::normalize;
use personal_groktor::report;
use personal_groktor::schema::{Annotation, Arm, ExperimentStatus};
use personal_groktor::store::Store;
use personal_groktor::GroktorError;
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(
    name = "personal_groktor",
    version,
    about = "Turn personal health exports into clear, actionable wellbeing insight"
)]
struct Cli {
    /// Path to SQLite database (default: ~/.local/share/personal_groktor/data.db)
    #[arg(long, global = true, env = "GROKTOR_DB")]
    db: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Import a CSV or JSON health export into the local database
    Ingest {
        /// Path to .csv or .json export
        path: PathBuf,
    },

    /// Run rule-based anomaly detection on stored metrics
    Analyze {
        /// Start day (YYYY-MM-DD); default: earliest in DB
        #[arg(long)]
        from: Option<NaiveDate>,
        /// End day (YYYY-MM-DD); default: latest in DB
        #[arg(long)]
        to: Option<NaiveDate>,
    },

    /// Daily/weekly wellbeing brief (rules + context + optional Grok narrative)
    Brief {
        /// Day to summarize (YYYY-MM-DD); default: latest day with data
        #[arg(long)]
        day: Option<NaiveDate>,
        /// Rolling 7-day week ending on --day
        #[arg(long)]
        week: bool,
        /// Call xAI Grok for a narrative (requires XAI_API_KEY)
        #[arg(long)]
        llm: bool,
        /// Write markdown report to this path
        #[arg(long)]
        out: Option<PathBuf>,
        /// Recompute and overwrite digest cache (default: always recompute in v0)
        #[arg(long)]
        refresh: bool,
    },

    /// Alias for `brief` (compat)
    Digest {
        #[arg(long)]
        day: Option<NaiveDate>,
        #[arg(long)]
        llm: bool,
        #[arg(long)]
        out: Option<PathBuf>,
    },

    /// Log or list personal annotations (notes/tags)
    Note {
        #[command(subcommand)]
        action: Option<NoteCmd>,
        /// Tags for a new note (repeatable)
        #[arg(long = "tag", short = 't')]
        tags: Vec<String>,
        /// Day (YYYY-MM-DD); default: today
        #[arg(long)]
        day: Option<NaiveDate>,
        /// Free-text body
        #[arg(short = 'm', long = "message")]
        message: Option<String>,
        /// Mood 1–5
        #[arg(long)]
        mood: Option<u8>,
        /// Energy 1–5
        #[arg(long)]
        energy: Option<u8>,
        /// Link to experiment slug
        #[arg(long)]
        experiment: Option<String>,
        /// With --experiment: also set arm for the day
        #[arg(long)]
        arm: Option<String>,
    },

    /// N=1 self-experiments
    Lab {
        #[command(subcommand)]
        action: LabCmd,
    },

    /// Show database status
    Status,
}

#[derive(Debug, Subcommand)]
enum NoteCmd {
    /// List annotations
    List {
        #[arg(long)]
        from: Option<NaiveDate>,
        #[arg(long)]
        to: Option<NaiveDate>,
        #[arg(long)]
        tag: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
enum LabCmd {
    /// Start a new active experiment
    Start {
        slug: String,
        #[arg(long)]
        title: String,
        #[arg(long)]
        hypothesis: String,
        /// Outcome specs: kind:direction (e.g. hrv:up). Repeatable.
        #[arg(long = "outcome", required = true)]
        outcomes: Vec<String>,
        /// Primary outcome kind alias (e.g. hrv)
        #[arg(long)]
        primary: Option<String>,
        #[arg(long, default_value_t = 14)]
        min_days: u32,
        #[arg(long)]
        started_on: Option<NaiveDate>,
        #[arg(long)]
        notes: Option<String>,
    },
    Pause {
        slug: String,
    },
    Resume {
        slug: String,
    },
    Complete {
        slug: String,
    },
    Abandon {
        slug: String,
        #[arg(long)]
        reason: Option<String>,
    },
    /// Assign arm for a day or date range
    Day {
        slug: String,
        #[arg(long)]
        day: Option<NaiveDate>,
        #[arg(long)]
        from: Option<NaiveDate>,
        #[arg(long)]
        to: Option<NaiveDate>,
        #[arg(long)]
        arm: String,
        #[arg(long)]
        note: Option<String>,
    },
    List,
    Show {
        slug: String,
    },
    /// Descriptive N=1 report
    Report {
        slug: String,
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

fn default_db_path() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("personal_groktor")
        .join("data.db")
}

fn open_store(db: Option<PathBuf>) -> personal_groktor::Result<Store> {
    let path = db.unwrap_or_else(default_db_path);
    Store::open(path)
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_target(false)
        .init();

    if let Err(e) = run().await {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

async fn run() -> personal_groktor::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Ingest { path } => cmd_ingest(cli.db, path),
        Commands::Analyze { from, to } => cmd_analyze(cli.db, from, to),
        Commands::Brief {
            day,
            week,
            llm,
            out,
            refresh,
        } => cmd_brief(cli.db, BriefOptions { day, week, refresh }, llm, out).await,
        Commands::Digest { day, llm, out } => {
            cmd_brief(
                cli.db,
                BriefOptions {
                    day,
                    week: false,
                    refresh: true,
                },
                llm,
                out,
            )
            .await
        }
        Commands::Note {
            action,
            tags,
            day,
            message,
            mood,
            energy,
            experiment,
            arm,
        } => cmd_note(
            cli.db, action, tags, day, message, mood, energy, experiment, arm,
        ),
        Commands::Lab { action } => cmd_lab(cli.db, action),
        Commands::Status => cmd_status(cli.db),
    }
}

fn cmd_ingest(db: Option<PathBuf>, path: PathBuf) -> personal_groktor::Result<()> {
    let store = open_store(db)?;
    let raw = ingest::load_path(&path)?;
    if raw.is_empty() {
        println!("No rows found in {}", path.display());
        return Ok(());
    }
    let (points, errors) = normalize::normalize_all(&raw);
    for e in &errors {
        tracing::warn!("{e}");
    }
    let inserted = store.upsert_metrics(&points)?;
    println!(
        "Ingested {} row(s) from {} → {} new metric(s) stored ({} parse error(s))",
        raw.len(),
        path.display(),
        inserted,
        errors.len()
    );
    println!("Database: {}", store.path().display());
    Ok(())
}

fn cmd_analyze(
    db: Option<PathBuf>,
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
) -> personal_groktor::Result<()> {
    let store = open_store(db)?;
    let range = store.day_range()?;
    let Some((min_d, max_d)) = range else {
        println!("No metrics in database. Run `ingest` first.");
        return Ok(());
    };
    let start = from.unwrap_or(min_d);
    let end = to.unwrap_or(max_d);
    let points = store.metrics_in_range(start, end)?;
    let findings = analyze::analyze(&points);
    let days: Vec<_> = analyze::unique_days(&points);
    store.replace_findings_for_days(&days, &findings)?;
    println!(
        "Analyzed {} metric(s) from {start} to {end} → {} finding(s)",
        points.len(),
        findings.len()
    );
    if !findings.is_empty() {
        println!();
        print!("{}", report::format_findings_table(&findings));
    }
    Ok(())
}

async fn cmd_brief(
    db: Option<PathBuf>,
    opts: BriefOptions,
    use_llm: bool,
    out: Option<PathBuf>,
) -> personal_groktor::Result<()> {
    let store = open_store(db)?;
    let mut digest = brief::build_brief(&store, &opts)?;

    if use_llm {
        let day_metrics = brief::metrics_for_digest(&store, &digest)?;
        let client = GrokClient::from_env()?;
        // Prefer extended prompt with annotations when available.
        let prompt = brief::build_brief_prompt(&digest, &day_metrics);
        match client.complete_raw(&prompt).await {
            Ok(text) => {
                digest.llm_narrative = Some(text);
                digest.llm_backend = Some("grok".into());
            }
            Err(e) => {
                // Fall back to classic narrate if complete_raw missing path fails oddly
                tracing::warn!("Grok narrative failed: {e}");
                match client.narrate(&day_metrics, &digest.findings).await {
                    Ok(text) => {
                        digest.llm_narrative = Some(text);
                        digest.llm_backend = Some("grok".into());
                    }
                    Err(e2) => tracing::warn!("Grok narrative failed: {e2}"),
                }
            }
        }
    }

    let _ = store.upsert_digest(&digest);
    let text = report::format_digest(&digest);
    if let Some(path) = out {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, &text)?;
        println!("Wrote brief to {}", path.display());
    }
    print!("{text}");
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn cmd_note(
    db: Option<PathBuf>,
    action: Option<NoteCmd>,
    tags: Vec<String>,
    day: Option<NaiveDate>,
    message: Option<String>,
    mood: Option<u8>,
    energy: Option<u8>,
    experiment: Option<String>,
    arm: Option<String>,
) -> personal_groktor::Result<()> {
    let store = open_store(db)?;

    if let Some(NoteCmd::List { from, to, tag }) = action {
        let start = from.unwrap_or_else(|| NaiveDate::from_ymd_opt(1970, 1, 1).unwrap());
        let end = to.unwrap_or_else(|| Utc::now().date_naive());
        let notes = if let Some(t) = tag {
            store.annotations_with_tag(&t, Some(start), Some(end))?
        } else {
            store.annotations_in_range(start, end)?
        };
        if notes.is_empty() {
            println!("No annotations in range.");
            return Ok(());
        }
        for a in notes {
            let tags = a.tags.join(", ");
            let body = a.body.as_deref().unwrap_or("");
            println!("{}  [{}]  {body}", a.day, tags);
        }
        return Ok(());
    }

    // Default: add
    if tags.is_empty() && message.is_none() && mood.is_none() && energy.is_none() {
        return Err(GroktorError::Message(
            "usage: note --tag TAG [-m message] [--day DATE] | note list".into(),
        ));
    }

    let day = day.unwrap_or_else(|| Utc::now().date_naive());
    validate_scale("mood", mood)?;
    validate_scale("energy", energy)?;

    let mut ann = Annotation::new(day, tags, message);
    ann.mood = mood;
    ann.energy = energy;

    if let Some(slug) = &experiment {
        let exp = store.require_experiment(slug)?;
        ann.experiment_id = Some(exp.id);
        if let Some(arm_s) = &arm {
            let arm = Arm::parse(arm_s).ok_or_else(|| {
                GroktorError::Parse(format!(
                    "unknown arm `{arm_s}` (use intervention|control|exclude)"
                ))
            })?;
            lab::assign_day(&store, slug, day, arm, None)?;
        }
    } else if arm.is_some() {
        return Err(GroktorError::Message("--arm requires --experiment".into()));
    }

    store.insert_annotation(&ann)?;
    println!(
        "Logged note on {day}: tags=[{}] id={}",
        ann.tags.join(", "),
        ann.id
    );
    Ok(())
}

fn validate_scale(name: &str, v: Option<u8>) -> personal_groktor::Result<()> {
    if let Some(n) = v {
        if !(1..=5).contains(&n) {
            return Err(GroktorError::Parse(format!("{name} must be 1–5 (got {n})")));
        }
    }
    Ok(())
}

fn cmd_lab(db: Option<PathBuf>, action: LabCmd) -> personal_groktor::Result<()> {
    let store = open_store(db)?;
    match action {
        LabCmd::Start {
            slug,
            title,
            hypothesis,
            outcomes,
            primary,
            min_days,
            started_on,
            notes,
        } => {
            let mut specs = Vec::new();
            for o in &outcomes {
                specs.push(lab::parse_outcome_spec(o)?);
            }
            let exp = lab::start_experiment(
                &store,
                &slug,
                &title,
                &hypothesis,
                specs,
                primary.as_deref(),
                min_days,
                started_on,
                notes,
            )?;
            println!(
                "Started experiment `{}` (status={}, min_days={})",
                exp.slug, exp.status, exp.min_days
            );
            for o in &exp.outcomes {
                let p = if o.primary { " primary" } else { "" };
                println!("  outcome: {} {}{p}", o.kind.as_str(), o.direction);
            }
        }
        LabCmd::Pause { slug } => {
            let exp = lab::set_status(&store, &slug, ExperimentStatus::Paused)?;
            println!("Paused `{}`", exp.slug);
        }
        LabCmd::Resume { slug } => {
            let exp = lab::set_status(&store, &slug, ExperimentStatus::Active)?;
            println!("Resumed `{}`", exp.slug);
        }
        LabCmd::Complete { slug } => {
            let exp = lab::set_status(&store, &slug, ExperimentStatus::Completed)?;
            println!("Completed `{}`", exp.slug);
        }
        LabCmd::Abandon { slug, reason } => {
            let mut exp = lab::set_status(&store, &slug, ExperimentStatus::Abandoned)?;
            if let Some(r) = reason {
                let note = match exp.notes {
                    Some(n) => format!("{n}\nabandoned: {r}"),
                    None => format!("abandoned: {r}"),
                };
                exp.notes = Some(note);
                exp.updated_at = Utc::now();
                store.update_experiment(&exp)?;
            }
            println!("Abandoned `{}`", exp.slug);
        }
        LabCmd::Day {
            slug,
            day,
            from,
            to,
            arm,
            note,
        } => {
            let arm = Arm::parse(&arm).ok_or_else(|| {
                GroktorError::Parse(format!(
                    "unknown arm `{arm}` (use intervention|control|exclude)"
                ))
            })?;
            if let (Some(f), Some(t)) = (from, to) {
                let n = lab::assign_range(&store, &slug, f, t, arm, note)?;
                println!("Assigned {arm} to {n} day(s) on `{slug}` ({f} → {t})");
            } else if let Some(f) = from {
                let t = to.unwrap_or(f);
                let n = lab::assign_range(&store, &slug, f, t, arm, note)?;
                println!("Assigned {arm} to {n} day(s) on `{slug}`");
            } else {
                let day = day.unwrap_or_else(|| Utc::now().date_naive());
                lab::assign_day(&store, &slug, day, arm, note)?;
                println!("Assigned {arm} on {day} for `{slug}`");
            }
        }
        LabCmd::List => {
            let exps = store.list_experiments()?;
            if exps.is_empty() {
                println!("No experiments. Start one with `lab start`.");
                return Ok(());
            }
            for e in exps {
                let start = e
                    .started_on
                    .map(|d| d.to_string())
                    .unwrap_or_else(|| "—".into());
                println!(
                    "{:16} {:10}  started {start}  — {}",
                    e.slug, e.status, e.title
                );
            }
        }
        LabCmd::Show { slug } => {
            let exp = store.require_experiment(&slug)?;
            let days = store.experiment_days(exp.id)?;
            println!("slug:        {}", exp.slug);
            println!("title:       {}", exp.title);
            println!("status:      {}", exp.status);
            println!("hypothesis:  {}", exp.hypothesis);
            println!(
                "started_on:  {}",
                exp.started_on
                    .map(|d| d.to_string())
                    .unwrap_or_else(|| "—".into())
            );
            println!(
                "ended_on:    {}",
                exp.ended_on
                    .map(|d| d.to_string())
                    .unwrap_or_else(|| "—".into())
            );
            println!("min_days:    {}", exp.min_days);
            println!("outcomes:");
            for o in &exp.outcomes {
                let p = if o.primary { " (primary)" } else { "" };
                println!("  - {} {}{p}", o.kind.as_str(), o.direction);
            }
            let n_i = days.iter().filter(|d| d.arm == Arm::Intervention).count();
            let n_c = days.iter().filter(|d| d.arm == Arm::Control).count();
            let n_x = days.iter().filter(|d| d.arm == Arm::Exclude).count();
            println!("arm days:    intervention={n_i} control={n_c} exclude={n_x}");
            if let Some(n) = &exp.notes {
                println!("notes:       {n}");
            }
        }
        LabCmd::Report { slug, out } => {
            let report = lab::build_lab_report(&store, &slug)?;
            let text = report::format_lab_report(&report);
            if let Some(path) = out {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&path, &text)?;
                println!("Wrote lab report to {}", path.display());
            }
            print!("{text}");
        }
    }
    Ok(())
}

fn cmd_status(db: Option<PathBuf>) -> personal_groktor::Result<()> {
    let store = open_store(db)?;
    let count = store.metric_count()?;
    let findings = store.finding_count()?;
    let notes = store.annotation_count()?;
    let (active, completed) = store.experiment_counts()?;
    println!("Database:  {}", store.path().display());
    println!("Metrics:   {count}");
    match store.day_range()? {
        Some((a, b)) => println!("Range:     {a} → {b}"),
        None => println!("Range:     (empty)"),
    }
    println!("Findings:  {findings}");
    println!("Notes:     {notes}");
    println!("Labs:      {active} active, {completed} completed");
    Ok(())
}
