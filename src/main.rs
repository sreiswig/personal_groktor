//! Personal Groktor CLI.

use std::path::PathBuf;

use chrono::NaiveDate;
use clap::{Parser, Subcommand};
use personal_groktor::analyze;
use personal_groktor::ingest;
use personal_groktor::llm::{self, GrokClient};
use personal_groktor::normalize;
use personal_groktor::report;
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

    /// Print a daily wellbeing digest (rules + optional Grok narrative)
    Digest {
        /// Day to summarize (YYYY-MM-DD); default: latest day with data
        #[arg(long)]
        day: Option<NaiveDate>,
        /// Call xAI Grok for a narrative (requires XAI_API_KEY)
        #[arg(long)]
        llm: bool,
        /// Write markdown report to this path
        #[arg(long)]
        out: Option<PathBuf>,
    },

    /// Show database status
    Status,
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
        Commands::Digest { day, llm, out } => cmd_digest(cli.db, day, llm, out).await,
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

async fn cmd_digest(
    db: Option<PathBuf>,
    day: Option<NaiveDate>,
    use_llm: bool,
    out: Option<PathBuf>,
) -> personal_groktor::Result<()> {
    let store = open_store(db)?;
    let day = report::resolve_day(day, store.day_range()?).ok_or_else(|| {
        GroktorError::Message("No metrics in database. Run `ingest` first.".into())
    })?;

    // Ensure findings exist for a reasonable window around the day
    let window_start = day - chrono::Duration::days(21);
    let points = store.metrics_in_range(window_start, day)?;
    let all_findings = analyze::analyze(&points);
    let days = analyze::unique_days(&points);
    store.replace_findings_for_days(&days, &all_findings)?;

    let day_findings: Vec<_> = all_findings
        .into_iter()
        .filter(|f| f.day == day)
        .collect();
    let day_metrics: Vec<_> = points.into_iter().filter(|p| p.day == day).collect();
    let metric_count = day_metrics.len();

    let llm_narrative = if use_llm {
        let client = GrokClient::from_env()?;
        match client.narrate(&day_metrics, &day_findings).await {
            Ok(text) => Some(text),
            Err(e) => {
                tracing::warn!("Grok narrative failed: {e}");
                None
            }
        }
    } else {
        None
    };

    let digest = llm::digest_from_parts(day, day_findings, metric_count, llm_narrative);
    let text = report::format_digest(&digest);
    if let Some(path) = out {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, &text)?;
        println!("Wrote digest to {}", path.display());
    }
    print!("{text}");
    Ok(())
}

fn cmd_status(db: Option<PathBuf>) -> personal_groktor::Result<()> {
    let store = open_store(db)?;
    let count = store.metric_count()?;
    println!("Database: {}", store.path().display());
    println!("Metrics:  {count}");
    match store.day_range()? {
        Some((a, b)) => println!("Range:    {a} → {b}"),
        None => println!("Range:    (empty)"),
    }
    Ok(())
}
