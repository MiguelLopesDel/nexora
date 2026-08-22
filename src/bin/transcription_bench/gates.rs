//! Pass/fail gates: transcription WER and latency, plus the question-
//! answering stage and its own keyword/latency gates.

use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use nexora::config::{ProviderConfig, ProviderKind};
use nexora::conversation::Role;
use nexora::providers::{ChatRequest, StreamEvent, stream_chat};
use nexora::ui::append_meeting_transcript_context;

use crate::Cli;
use crate::fixtures::{FIXTURES, FixtureResult, fold};

struct Answer {
    text: String,
    first_token: Duration,
    total: Duration,
}

async fn ask_model(cli: &Cli, question: &str, live_transcript: &[String]) -> Result<Answer> {
    let provider = ProviderConfig {
        kind: ProviderKind::Openai,
        base_url: Some(cli.qa_url.clone()),
        api_key: Some("ollama".into()),
        api_key_env: None,
        default_model: Some(cli.qa_model.clone()),
        thinking: None,
        reasoning_effort: None,
    };
    let mut messages = vec![(Role::User, question.to_string())];
    append_meeting_transcript_context(&mut messages, live_transcript, 12_000);
    let request = ChatRequest {
        model: cli.qa_model.clone(),
        system: Some(
            "You are Nexora, a concise on-screen assistant. Answer briefly and directly.".into(),
        ),
        messages,
        image_png: None,
        max_tokens: 512,
    };
    let (tx, rx) = async_channel::unbounded::<StreamEvent>();
    let started = Instant::now();
    tokio::spawn(async move { stream_chat(&provider, request, tx).await });
    let mut text = String::new();
    let mut first_token = None;
    while let Ok(event) = rx.recv().await {
        match event {
            StreamEvent::Delta(delta) => {
                if first_token.is_none() && !delta.trim().is_empty() {
                    first_token = Some(started.elapsed());
                }
                text.push_str(&delta);
            }
            StreamEvent::Done => break,
            StreamEvent::Error(message) => bail!("model error: {message}"),
        }
    }
    Ok(Answer {
        text: text.trim().to_string(),
        first_token: first_token.unwrap_or_else(|| started.elapsed()),
        total: started.elapsed(),
    })
}

fn keywords_found(answer: &str, groups: &[&[&str]]) -> Vec<bool> {
    let folded = fold(answer);
    groups
        .iter()
        .map(|group| group.iter().any(|keyword| folded.contains(&fold(keyword))))
        .collect()
}

fn mean_ms(durations: impl Iterator<Item = Duration>) -> Option<u128> {
    let values: Vec<u128> = durations.map(|d| d.as_millis()).collect();
    (!values.is_empty()).then(|| values.iter().sum::<u128>() / values.len() as u128)
}

/// Print the WER and latency gates and return whether both passed.
pub(crate) fn print_quality_gates(results: &[FixtureResult], cli: &Cli) -> bool {
    let mean_wer =
        results.iter().map(|r| r.corrected_wer).sum::<f64>() / results.len().max(1) as f64;
    let worst = results
        .iter()
        .max_by(|a, b| a.corrected_wer.total_cmp(&b.corrected_wer));
    let mean_latency = mean_ms(
        results
            .iter()
            .flat_map(|r| r.updates.iter().map(|u| u.latency)),
    )
    .unwrap_or(0);
    let mut pass = true;

    println!("== transcription quality ==");
    println!(
        "mean corrected WER {:.1}% (gate ≤ {:.0}%)",
        mean_wer * 100.0,
        cli.max_wer * 100.0
    );
    if let Some(worst) = worst {
        println!(
            "worst fixture {} at {:.1}% (gate ≤ {:.0}%)",
            worst.id,
            worst.corrected_wer * 100.0,
            cli.max_fixture_wer * 100.0
        );
    }
    if mean_wer > cli.max_wer {
        println!("FAIL: mean WER above gate");
        pass = false;
    }
    if let Some(worst) = worst
        && worst.corrected_wer > cli.max_fixture_wer
    {
        println!("FAIL: fixture {} above per-fixture gate", worst.id);
        pass = false;
    }

    println!("\n== latency ==");
    println!(
        "audio heard → text available: {} ms mean (gate ≤ {} ms); capture stride adds up to {} ms before that",
        mean_latency,
        cli.max_latency_ms,
        cli.chunk_seconds * 1_000
    );
    if mean_latency > cli.max_latency_ms {
        println!("FAIL: mean transcription latency above gate");
        pass = false;
    }
    pass
}

/// Ask each fixture's comprehension questions and print the keyword/latency
/// gates. Returns whether every question passed.
pub(crate) async fn run_qa_stage(cli: &Cli, results: &[FixtureResult]) -> bool {
    let mut pass = true;
    println!("\n== questions about what was heard ({}) ==", cli.qa_model);
    // First contact loads the model into memory; keep that cold-start out of
    // the per-question latency numbers (the app keeps models warm).
    let warmup = Instant::now();
    match ask_model(cli, "Reply with the single word: ready", &[]).await {
        Ok(_) => println!(
            "model warm-up: {} ms (excluded from gates)",
            warmup.elapsed().as_millis()
        ),
        Err(err) => println!("model warm-up failed: {err:#}"),
    }
    for (fixture, result) in FIXTURES.iter().zip(results) {
        for question in fixture.questions {
            println!("\n[{}] Q: {}", fixture.id, question.ask);
            match ask_model(cli, question.ask, &result.live_transcript).await {
                Ok(answer) => {
                    let found = keywords_found(&answer.text, question.keyword_groups);
                    let ok = found.iter().all(|hit| *hit);
                    println!("A: {}", answer.text);
                    println!(
                        "keywords {} · first token {} ms · total {} ms",
                        if ok { "OK" } else { "MISSING" },
                        answer.first_token.as_millis(),
                        answer.total.as_millis()
                    );
                    if !ok {
                        for (group, hit) in question.keyword_groups.iter().zip(&found) {
                            if !hit {
                                println!("  missing any of: {group:?}");
                            }
                        }
                        pass = false;
                    }
                    if answer.first_token.as_millis() > cli.max_qa_first_token_ms {
                        println!("FAIL: first token above gate");
                        pass = false;
                    }
                }
                Err(err) => {
                    println!("A: <error: {err:#}>");
                    pass = false;
                }
            }
        }
    }
    pass
}
