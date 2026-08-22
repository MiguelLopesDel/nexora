//! Developer benchmark for the live meeting pipeline.
//!
//! Plays synthetic speech (espeak-ng) into a dedicated PulseAudio null sink —
//! never the default output, so nothing is audible and real audio is never
//! mixed in — captures it back through `parec` exactly like the app, streams
//! it through the same rolling-window whisper transcription, and reports:
//!
//! - the exact transcript of every fixture, before and after
//!   [meeting.corrections]-style fixes,
//! - word error rate (WER) per fixture with a pass/fail accuracy gate,
//! - transcription latency (audio heard → text available) and inference cost,
//! - answers from a local Ollama model to questions about what was heard
//!   ("what is X?", "what did the speaker mean?"), with keyword checks and
//!   first-token/total latency.
//!
//! Exit code: 0 all gates pass · 1 a gate failed · 2 environment problem.

// Quality gate: no file over 500 lines, no function over 100 lines or
// cognitive complexity 25 (see clippy.toml). Split the module instead of
// allow()-ing this away.
#![warn(clippy::too_many_lines, clippy::cognitive_complexity)]

mod fixtures;
mod gates;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use clap::Parser;
use nexora::whisper::{ComputePreference, Transcriber, model_path};

use fixtures::{FIXTURES, FixtureResult, NullSink, default_piper_voices_dir, synthesize};
use gates::{print_quality_gates, run_qa_stage};

#[derive(Parser)]
#[command(
    name = "transcription_bench",
    about = "End-to-end transcription quality/latency benchmark on an isolated null sink"
)]
pub(crate) struct Cli {
    /// Curated whisper model id (tiny, base, small, large-v3-turbo-q5_0)
    #[arg(long, default_value = "tiny")]
    model: String,
    /// Force whisper language detection instead of per-fixture hints
    #[arg(long)]
    auto_language: bool,
    /// Capture stride in seconds (the app default is 2)
    #[arg(long, default_value_t = 2)]
    pub(crate) chunk_seconds: u64,
    /// Rolling transcription window in seconds (the app default is 4)
    #[arg(long, default_value_t = 4)]
    pub(crate) window_seconds: u64,
    /// Gate: maximum mean corrected WER across fixtures
    #[arg(long, default_value_t = 0.35)]
    pub(crate) max_wer: f64,
    /// Gate: maximum corrected WER for any single fixture
    #[arg(long, default_value_t = 0.60)]
    pub(crate) max_fixture_wer: f64,
    /// Gate: maximum mean latency from audio captured to text available (ms)
    #[arg(long, default_value_t = 2_500)]
    pub(crate) max_latency_ms: u128,
    /// Skip the question-answering stage
    #[arg(long)]
    skip_qa: bool,
    /// OpenAI-compatible endpoint for the QA stage
    #[arg(long, default_value = "http://localhost:11434/v1")]
    pub(crate) qa_url: String,
    /// Model used to answer questions about the transcript
    #[arg(long, default_value = "gemma4:e2b")]
    pub(crate) qa_model: String,
    /// Gate: maximum time to the first answer token (ms)
    #[arg(long, default_value_t = 30_000)]
    pub(crate) max_qa_first_token_ms: u128,
    /// Keep the generated wav files for listening/debugging
    #[arg(long)]
    keep_wavs: bool,
    /// Speech synthesizer: "auto" uses piper voices when installed (much more
    /// realistic than espeak) and falls back to espeak-ng per fixture.
    #[arg(long, default_value = "auto")]
    tts: String,
    /// Directory holding piper voices (<lang>.onnx as named in PIPER_VOICES)
    #[arg(long, default_value_os_t = default_piper_voices_dir())]
    piper_voices: PathBuf,
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    match run(&cli).await {
        Ok(true) => std::process::ExitCode::SUCCESS,
        Ok(false) => std::process::ExitCode::from(1),
        Err(err) => {
            eprintln!("transcription_bench: {err:#}");
            std::process::ExitCode::from(2)
        }
    }
}

async fn load_transcriber(cli: &Cli) -> Result<Arc<Transcriber>> {
    let model = model_path(&cli.model);
    if !model.exists() {
        bail!(
            "whisper model `{}` is not downloaded (expected {}); download it in the app or with the whisper manager",
            cli.model,
            model.display()
        );
    }
    let transcriber = Arc::new(
        tokio::task::spawn_blocking({
            let model = model.clone();
            move || Transcriber::load(&model, ComputePreference::Cpu)
        })
        .await
        .context("whisper load task failed")??,
    );
    println!(
        "model: {} ({}) · chunk {}s · window {}s",
        cli.model,
        transcriber.compute_label(),
        cli.chunk_seconds,
        cli.window_seconds
    );
    Ok(transcriber)
}

fn prepare_bench_environment() -> Result<(PathBuf, NullSink)> {
    let wav_dir = std::env::temp_dir().join(format!("nexora-bench-{}", std::process::id()));
    std::fs::create_dir_all(&wav_dir)?;
    let sink = NullSink::create()?;
    println!(
        "null sink: {} (isolated from the default output)\n",
        sink.name
    );
    Ok((wav_dir, sink))
}

async fn run_all_fixtures(
    cli: &Cli,
    wav_dir: &std::path::Path,
    sink: &NullSink,
    transcriber: &Arc<Transcriber>,
) -> Result<Vec<FixtureResult>> {
    let corrections = fixtures::bench_corrections();
    let mut results = Vec::new();
    for fixture in FIXTURES {
        let (wav, tts) = synthesize(fixture, wav_dir, cli)?;
        println!("▶ {} — playing + transcribing… (voice: {tts})", fixture.id);
        let result =
            fixtures::run_fixture(fixture, &wav, sink, transcriber, &corrections, cli).await?;
        println!("  expected : {}", result.expected);
        println!("  raw      : {}", result.raw_transcript);
        if result.corrected_transcript != result.raw_transcript {
            println!("  corrected: {}", result.corrected_transcript);
        }
        let latency = mean_ms(result.updates.iter().map(|u| u.latency)).unwrap_or(0);
        let inference = mean_ms(result.updates.iter().map(|u| u.inference)).unwrap_or(0);
        println!(
            "  WER {:.0}% raw → {:.0}% corrected · {} updates · latency {} ms mean (inference {} ms)\n",
            result.raw_wer * 100.0,
            result.corrected_wer * 100.0,
            result.updates.len(),
            latency,
            inference
        );
        results.push(result);
    }
    Ok(results)
}

fn mean_ms(durations: impl Iterator<Item = std::time::Duration>) -> Option<u128> {
    let values: Vec<u128> = durations.map(|d| d.as_millis()).collect();
    (!values.is_empty()).then(|| values.iter().sum::<u128>() / values.len() as u128)
}

fn cleanup_wavs(cli: &Cli, wav_dir: &std::path::Path) {
    if !cli.keep_wavs {
        let _ = std::fs::remove_dir_all(wav_dir);
    } else {
        println!("wav files kept in {}\n", wav_dir.display());
    }
}

async fn run(cli: &Cli) -> Result<bool> {
    let transcriber = load_transcriber(cli).await?;
    let (wav_dir, sink) = prepare_bench_environment()?;
    let results = run_all_fixtures(cli, &wav_dir, &sink, &transcriber).await?;
    cleanup_wavs(cli, &wav_dir);

    let mut pass = print_quality_gates(&results, cli);
    if !cli.skip_qa {
        let qa_pass = run_qa_stage(cli, &results).await;
        pass = pass && qa_pass;
    }

    println!("\n{}", if pass { "PASS" } else { "FAIL" });
    Ok(pass)
}
