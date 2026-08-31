//! `stk verify` -- check that lossless files really hold lossless audio.

use std::path::{Path, PathBuf};

use clap::Args;
use serde_json::json;

use crate::audio::{ffmpeg, verify::verify_files};
use crate::cli::context::{show, table, AppContext};
use crate::errors::{Error, Result};
use crate::library::scan::{iter_audio_files, Excludes, LOSSLESS_EXTENSIONS};
use crate::models::{Verdict, VerifyResult};

pub const REPORT_FIELDS: &[&str] = &[
    "path",
    "verdict",
    "detail",
    "codec",
    "sample_rate",
    "bit_depth",
    "cutoff_hz",
    "nyquist_hz",
    "steepness_db_per_khz",
];

#[derive(Args, Debug)]
pub struct VerifyArgs {
    /// File or folder to check. Defaults to the whole library.
    pub target: Option<PathBuf>,

    /// Write a CSV report here.
    #[arg(long)]
    pub report: Option<PathBuf>,

    /// Save a spectrogram beside each flagged file.
    #[arg(long)]
    pub spectrograms: bool,

    /// Print only SUSPECT and FAKE results.
    #[arg(long)]
    pub only_flagged: bool,
}

/// Analyse the frequency spectrum of every lossless file and report the verdicts.
pub fn run(context: &AppContext, args: &VerifyArgs) -> Result<i32> {
    let paths = targets(context, args.target.as_deref())?;
    if paths.is_empty() {
        context.say("No lossless files to check.");
        return Ok(0);
    }

    let results = verify_files(&paths, &context.settings().verify);

    if args.spectrograms {
        render_spectrograms(context, &results);
    }
    if let Some(destination) = &args.report {
        write_report(destination, &results)?;
    }

    if context.as_json {
        context.emit(&json!(results.iter().map(as_row).collect::<Vec<_>>()));
        return Ok(0);
    }

    print_table(context, &results, args.only_flagged);
    if let Some(destination) = &args.report {
        context.say(&format!("Report written to {}", show(destination)));
    }

    let any_fake = results.iter().any(|result| result.verdict == Verdict::Fake);
    Ok(i32::from(any_fake))
}

fn targets(context: &AppContext, target: Option<&Path>) -> Result<Vec<PathBuf>> {
    match target {
        None => {
            let exclude = context.excludes()?;
            Ok(context
                .require_library_roots()?
                .iter()
                .flat_map(|root| iter_audio_files(root, LOSSLESS_EXTENSIONS, &exclude))
                .collect())
        }
        Some(path) if path.is_file() => Ok(vec![path.to_path_buf()]),
        Some(path) if path.is_dir() => Ok(iter_audio_files(
            path,
            LOSSLESS_EXTENSIONS,
            &Excludes::default(),
        )),
        Some(path) => Err(Error::Config(format!(
            "no such file or folder: {}",
            show(path)
        ))),
    }
}

fn render_spectrograms(context: &AppContext, results: &[VerifyResult]) {
    for result in results {
        if !matches!(result.verdict, Verdict::Fake | Verdict::Suspect) {
            continue;
        }
        let destination = result.path.with_extension("spectrogram.png");
        if let Err(error) = ffmpeg::render_spectrogram(&result.path, &destination) {
            context.say(&format!("could not render {}: {error}", show(&destination)));
        }
    }
}

/// Write the CSV report.
///
/// `utf-8-sig` in the Python, so the BOM is written here too: without it Excel
/// mangles every non-ASCII path in the report.
fn write_report(destination: &Path, results: &[VerifyResult]) -> Result<()> {
    if let Some(parent) = destination.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|error| Error::Config(format!("{}: {error}", parent.display())))?;
        }
    }

    let mut buffer = b"\xef\xbb\xbf".to_vec();
    {
        let mut writer = csv::WriterBuilder::new()
            .terminator(csv::Terminator::CRLF)
            .from_writer(&mut buffer);
        writer
            .write_record(REPORT_FIELDS)
            .map_err(|error| Error::Config(error.to_string()))?;
        for result in results {
            writer
                .write_record(row_cells(result))
                .map_err(|error| Error::Config(error.to_string()))?;
        }
        writer
            .flush()
            .map_err(|error| Error::Config(error.to_string()))?;
    }

    std::fs::write(destination, buffer)
        .map_err(|error| Error::Config(format!("{}: {error}", destination.display())))
}

fn cell<T: ToString>(value: Option<T>) -> String {
    value.map(|inner| inner.to_string()).unwrap_or_default()
}

fn row_cells(result: &VerifyResult) -> Vec<String> {
    vec![
        show(&result.path),
        result.verdict.to_string(),
        result.detail.clone(),
        result.codec.clone(),
        cell(result.sample_rate),
        cell(result.bit_depth),
        cell(result.cutoff_hz),
        cell(result.nyquist_hz),
        // A float, so a whole number keeps its ".0" as Python's writer does.
        result
            .steepness_db_per_khz
            .map(|value| format!("{value:.1}"))
            .unwrap_or_default(),
    ]
}

fn as_row(result: &VerifyResult) -> serde_json::Value {
    json!({
        "path": show(&result.path),
        "verdict": result.verdict.to_string(),
        "detail": result.detail,
        "codec": result.codec,
        "sample_rate": result.sample_rate,
        "bit_depth": result.bit_depth,
        "cutoff_hz": result.cutoff_hz,
        "nyquist_hz": result.nyquist_hz,
        "steepness_db_per_khz": result.steepness_db_per_khz,
    })
}

fn print_table(context: &AppContext, results: &[VerifyResult], only_flagged: bool) {
    let shown: Vec<&VerifyResult> = results
        .iter()
        .filter(|result| {
            !only_flagged || matches!(result.verdict, Verdict::Fake | Verdict::Suspect)
        })
        .collect();

    if !shown.is_empty() {
        let rows: Vec<Vec<String>> = shown
            .iter()
            .map(|result| {
                vec![
                    result.verdict.to_string(),
                    result
                        .path
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    result.detail.clone(),
                ]
            })
            .collect();
        context.say(&table(&["Verdict", "File", "Detail"], &rows));
    }

    let counted: Vec<String> = Verdict::ALL
        .iter()
        .filter_map(|verdict| {
            let count = results
                .iter()
                .filter(|result| result.verdict == *verdict)
                .count();
            (count > 0).then(|| format!("{count} {verdict}"))
        })
        .collect();
    context.say(&format!(
        "Checked {} file(s): {}",
        results.len(),
        counted.join(", ")
    ));
}
