//! openqpcr command-line interface.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use openqpcr::model::SampleType;
use openqpcr::readers::{export, pcrd};

#[derive(Parser)]
#[command(
    name = "openqpcr",
    version,
    about = "Read Bio-Rad CFX (Connect / Duet / Opus) real-time PCR data and exports"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Print a human-readable summary of a run.
    Summary {
        /// A CFX export `.csv`/`.xlsx`, or a directory of export CSVs.
        path: PathBuf,
    },
    /// Dump the parsed run as JSON.
    Json {
        path: PathBuf,
        /// Pretty-print the JSON.
        #[arg(long)]
        pretty: bool,
    },
    /// Inspect the internal structure of a native `.pcrd` archive (reverse-engineering aid).
    Inspect { path: PathBuf },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Summary { path } => cmd_summary(&path),
        Command::Json { path, pretty } => cmd_json(&path, pretty),
        Command::Inspect { path } => cmd_inspect(&path),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn load(path: &std::path::Path) -> anyhow::Result<openqpcr::QpcrRun> {
    let mut run = if path.is_dir() {
        export::read_export_dir(path)?
    } else {
        openqpcr::read_path(path)?
    };
    // Derive Cq + melt peaks from raw curves when the source stored none (e.g. a
    // native `.pcrd`), matching what CFX computes on load.
    openqpcr::cq::annotate_run(&mut run, &openqpcr::cq::CqParams::default());
    openqpcr::cq::annotate_melt(&mut run, &openqpcr::cq::MeltParams::default());
    Ok(run)
}

fn cmd_summary(path: &std::path::Path) -> anyhow::Result<()> {
    let run = load(path)?;
    let m = &run.metadata;
    println!("== openqpcr run summary ==");
    if let Some(s) = &m.source_file {
        println!("source        : {s}");
    }
    if let Some(s) = &m.instrument {
        println!("instrument    : {s}");
    }
    if let Some(s) = &m.software_version {
        println!("software      : {s}");
    }
    if let Some(s) = &m.run_started {
        println!("run started   : {s}");
    }
    println!(
        "plate         : {}x{} ({} wells)",
        run.plate.rows,
        run.plate.cols,
        run.plate.well_count()
    );
    if let Some(c) = m.cycle_count {
        println!("cycles        : {c}");
    }

    let occupied = run.wells.len();
    let channels: usize = run.wells.iter().map(|w| w.channels.len()).sum();
    let fluors: std::collections::BTreeSet<&str> = run
        .wells
        .iter()
        .flat_map(|w| w.channels.iter())
        .map(|c| c.fluorophore.as_str())
        .collect();
    let with_melt = run
        .wells
        .iter()
        .flat_map(|w| w.channels.iter())
        .filter(|c| c.melt.is_some())
        .count();
    println!("occupied wells: {occupied}");
    println!("channels      : {channels}");
    println!(
        "fluorophores  : {}",
        fluors.into_iter().collect::<Vec<_>>().join(", ")
    );
    println!("melt curves   : {with_melt}");
    println!();

    // Per-well table.
    println!(
        "{:<5} {:<12} {:<10} {:<8} {:<8} {:>4} {:>5}",
        "Well", "Sample", "Fluor", "Target", "Type", "Cq", "Cyc"
    );
    for well in &run.wells {
        let sample = well.sample.clone().unwrap_or_default();
        let stype = fmt_type(well.sample_type);
        if well.channels.is_empty() {
            println!(
                "{:<5} {:<12} {:<10} {:<8} {:<8}",
                well.position(),
                trunc(&sample, 12),
                "",
                "",
                stype
            );
        }
        for ch in &well.channels {
            let target = ch.target.clone().unwrap_or_default();
            let cq = ch
                .cq
                .map(|v| format!("{v:.2}"))
                .unwrap_or_else(|| "-".into());
            println!(
                "{:<5} {:<12} {:<10} {:<8} {:<8} {:>4} {:>5}",
                well.position(),
                trunc(&sample, 12),
                trunc(&ch.fluorophore, 10),
                trunc(&target, 8),
                stype,
                cq,
                ch.amplification.len()
            );
        }
    }
    Ok(())
}

fn cmd_json(path: &std::path::Path, pretty: bool) -> anyhow::Result<()> {
    let run = load(path)?;
    let s = if pretty {
        serde_json::to_string_pretty(&run)?
    } else {
        serde_json::to_string(&run)?
    };
    println!("{s}");
    Ok(())
}

fn cmd_inspect(path: &std::path::Path) -> anyhow::Result<()> {
    if !pcrd::looks_like_zip(path)? {
        anyhow::bail!(
            "{} is not a ZIP-based .pcrd archive (no PK magic)",
            path.display()
        );
    }
    let insp = pcrd::inspect(path)?;
    println!("== .pcrd archive inspection: {} ==", path.display());
    println!("encrypted: {}", insp.encrypted);
    println!("entries  : {}", insp.entries.len());
    println!();
    println!(
        "{:<44} {:>12} {:>12} {:<8} enc",
        "name", "size", "compressed", "kind"
    );
    for e in &insp.entries {
        println!(
            "{:<44} {:>12} {:>12} {:<8?} {}",
            trunc(&e.name, 44),
            e.size,
            e.compressed_size,
            e.kind,
            if e.is_encrypted { "yes" } else { "" }
        );
    }
    Ok(())
}

fn fmt_type(t: SampleType) -> &'static str {
    match t {
        SampleType::Unknown => "Unknown",
        SampleType::Standard => "Standard",
        SampleType::Ntc => "NTC",
        SampleType::Nrt => "NRT",
        SampleType::PositiveControl => "Pos Ctrl",
        SampleType::NegativeControl => "Neg Ctrl",
        SampleType::Empty => "Empty",
    }
}

fn trunc(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(n.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}
