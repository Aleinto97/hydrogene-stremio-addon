use anyhow::{Context, Result};
use hydrogene::matching::{extract_season, extract_year};
use hydrogene::utils::is_exact_episode_match;
use once_cell::sync::Lazy;
use regex::Regex;
use reqwest::Client;
use serde::Deserialize;
use serde_json::Value;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

static COMPLETE_PACK_REGEX: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?i)\b(?:complete|season\s*\d{1,2}\s*complete|complete\s*season|全集|batch|pack)\b",
    )
    .expect("invalid complete pack regex")
});

static EXPLICIT_EP_REGEX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)\bS\d{1,2}E\d{1,3}\b").expect("invalid episode regex"));

static ANIME_EP_REGEX: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\b(?:E|EP|EPISODE)\s*\d{1,3}\b").expect("invalid anime episode regex")
});

#[derive(Debug, Deserialize)]
struct Case {
    label: String,
    content_type: String,
    stremio_id: String,
    #[serde(default)]
    title_all: Vec<String>,
    #[serde(default)]
    title_any: Vec<String>,
    #[serde(default)]
    forbidden_contains: Vec<String>,
    #[serde(default)]
    expected_year: Option<u32>,
    #[serde(default)]
    require_year_match: bool,
    #[serde(default)]
    expected_season: Option<u32>,
    #[serde(default)]
    expected_episode: Option<u32>,
    #[serde(default = "default_true")]
    accept_season_pack: bool,
    #[serde(default = "default_top_k")]
    top_k: usize,
    #[serde(default = "default_min_streams")]
    min_streams: usize,
}

#[derive(Debug, Deserialize)]
struct StreamResponse {
    streams: Vec<AddonStream>,
}

#[derive(Debug, Deserialize)]
struct AddonStream {
    title: String,
    #[serde(rename = "behaviorHints")]
    behavior_hints: Value,
}

#[derive(Debug)]
struct StreamEvaluation {
    filename: String,
    exact_episode: bool,
    acceptable: bool,
    conflicting: bool,
    reasons: Vec<String>,
}

#[derive(Default)]
struct Summary {
    cases: usize,
    top1_pass: usize,
    topk_pass: usize,
    no_streams: usize,
    episode_cases: usize,
    episode_top1_exact: usize,
    episode_topk_exact: usize,
    severe_false_positives: usize,
    ci_failures: usize,
}

fn default_true() -> bool {
    true
}

fn default_top_k() -> usize {
    3
}

fn default_min_streams() -> usize {
    1
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();

    let base_url = env::args()
        .nth(1)
        .or_else(|| env::var("STREMIO_ADDON_URL").ok())
        .unwrap_or_else(|| "http://127.0.0.1:8080".to_string());

    let cases_path = env::args()
        .nth(2)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("examples/data/stremio_accuracy_cases.json"));

    let cases = load_cases(&cases_path)?;
    let client = Client::builder()
        .timeout(Duration::from_secs(30))
        .pool_max_idle_per_host(10)
        .build()?;

    println!("=== STREMIO STREAM ACCURACY ===");
    println!("Base URL: {}", base_url);
    println!("Cases: {}\n", cases_path.display());

    let mut summary = Summary::default();

    for case in &cases {
        summary.cases += 1;
        let result = run_case(&client, &base_url, case).await?;
        let top_k = case.top_k.max(1);

        let top1_pass = result
            .evaluations
            .first()
            .map(|stream| stream.acceptable)
            .unwrap_or(false);
        let topk_pass = result
            .evaluations
            .iter()
            .take(top_k)
            .any(|stream| stream.acceptable);
        let top1_conflicting = result
            .evaluations
            .first()
            .map(|stream| stream.conflicting)
            .unwrap_or(false);

        if result.stream_count == 0 {
            summary.no_streams += 1;
        }
        if top1_pass {
            summary.top1_pass += 1;
        }
        if topk_pass {
            summary.topk_pass += 1;
        }
        if top1_conflicting {
            summary.severe_false_positives += 1;
        }

        if case.expected_episode.is_some() && case.expected_season.is_some() {
            summary.episode_cases += 1;
            if result
                .evaluations
                .first()
                .map(|stream| stream.exact_episode)
                .unwrap_or(false)
            {
                summary.episode_top1_exact += 1;
            }
            if result
                .evaluations
                .iter()
                .take(top_k)
                .any(|stream| stream.exact_episode)
            {
                summary.episode_topk_exact += 1;
            }
        }

        let ci_failed = result.stream_count < case.min_streams || !topk_pass;
        if ci_failed {
            summary.ci_failures += 1;
        }

        print_case_report(case, &result, top1_pass, topk_pass, ci_failed);
    }

    print_summary(&summary);

    if summary.ci_failures > 0 {
        std::process::exit(1);
    }

    Ok(())
}

#[derive(Debug)]
struct CaseResult {
    request_url: String,
    stream_count: usize,
    evaluations: Vec<StreamEvaluation>,
}

async fn run_case(client: &Client, base_url: &str, case: &Case) -> Result<CaseResult> {
    let encoded_id = urlencoding::encode(&case.stremio_id);
    let request_url = format!(
        "{}/stream/{}/{}.json",
        base_url.trim_end_matches('/'),
        case.content_type,
        encoded_id
    );

    let response = client
        .get(&request_url)
        .send()
        .await
        .with_context(|| format!("request failed for {}", request_url))?
        .error_for_status()
        .with_context(|| format!("non-success status for {}", request_url))?;

    let payload: StreamResponse = response
        .json()
        .await
        .with_context(|| format!("invalid JSON response for {}", request_url))?;

    let evaluations = payload
        .streams
        .into_iter()
        .map(|stream| evaluate_stream(case, &stream))
        .collect::<Vec<_>>();

    Ok(CaseResult {
        request_url,
        stream_count: evaluations.len(),
        evaluations,
    })
}

fn evaluate_stream(case: &Case, stream: &AddonStream) -> StreamEvaluation {
    let filename = stream
        .behavior_hints
        .get("filename")
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .unwrap_or_else(|| stream.title.lines().next().unwrap_or("").to_string());

    let normalized_filename = normalize(&filename);
    let mut reasons = Vec::new();
    let mut conflicting = false;

    let title_all_ok = case
        .title_all
        .iter()
        .all(|needle| contains_normalized(&normalized_filename, needle));
    if !title_all_ok {
        let missing = case
            .title_all
            .iter()
            .filter(|needle| !contains_normalized(&normalized_filename, needle))
            .cloned()
            .collect::<Vec<_>>();
        reasons.push(format!("missing={}", missing.join(",")));
    }

    let title_any_ok = case.title_any.is_empty()
        || case
            .title_any
            .iter()
            .any(|needle| contains_normalized(&normalized_filename, needle));
    if !title_any_ok {
        reasons.push(format!("need_any={}", case.title_any.join(",")));
    }

    let forbidden_hits = case
        .forbidden_contains
        .iter()
        .filter(|needle| contains_normalized(&normalized_filename, needle))
        .cloned()
        .collect::<Vec<_>>();
    if !forbidden_hits.is_empty() {
        conflicting = true;
        reasons.push(format!("forbidden={}", forbidden_hits.join(",")));
    }

    let extracted_year = extract_year(&filename);
    let year_ok = match case.expected_year {
        Some(expected_year) if case.require_year_match => extracted_year == Some(expected_year),
        Some(expected_year) => extracted_year.is_none() || extracted_year == Some(expected_year),
        None => true,
    };
    if !year_ok {
        conflicting = true;
        reasons.push(format!(
            "year={:?},expected={}",
            extracted_year,
            case.expected_year.unwrap_or_default()
        ));
    }

    let mut exact_episode = false;
    let episode_ok = match (case.expected_season, case.expected_episode) {
        (Some(season), Some(episode)) => {
            exact_episode = is_exact_episode_match(&filename, season, episode);
            let acceptable = exact_episode
                || (case.accept_season_pack && is_season_pack_candidate(&filename, season));
            if !acceptable {
                let found_season = extract_season(&filename);
                conflicting |= found_season == Some(season);
                reasons.push(format!("episode_mismatch=s{:02}e{:02}", season, episode));
            }
            acceptable
        }
        _ => true,
    };

    let acceptable =
        title_all_ok && title_any_ok && forbidden_hits.is_empty() && year_ok && episode_ok;
    if acceptable {
        reasons.push("ok".to_string());
    }

    StreamEvaluation {
        filename,
        exact_episode,
        acceptable,
        conflicting,
        reasons,
    }
}

fn is_season_pack_candidate(title: &str, expected_season: u32) -> bool {
    match extract_season(title) {
        Some(found_season) if found_season == expected_season => {
            COMPLETE_PACK_REGEX.is_match(title)
                || (!EXPLICIT_EP_REGEX.is_match(title) && !ANIME_EP_REGEX.is_match(title))
        }
        _ => false,
    }
}

fn normalize(value: &str) -> String {
    value
        .to_lowercase()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn contains_normalized(normalized_haystack: &str, needle: &str) -> bool {
    normalized_haystack.contains(&normalize(needle))
}

fn load_cases(path: &Path) -> Result<Vec<Case>> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed reading cases from {}", path.display()))?;
    serde_json::from_str(&raw)
        .with_context(|| format!("failed parsing JSON cases from {}", path.display()))
}

fn print_case_report(
    case: &Case,
    result: &CaseResult,
    top1_pass: bool,
    topk_pass: bool,
    ci_failed: bool,
) {
    let top_k = case.top_k.max(1);
    println!(
        "{} {}",
        if ci_failed { "[FAIL]" } else { "[PASS]" },
        case.label
    );
    println!("  request={} {}", case.content_type, result.request_url);
    println!(
        "  streams={} min={} top1={} top{}={}",
        result.stream_count, case.min_streams, top1_pass, top_k, topk_pass
    );

    if let (Some(season), Some(episode)) = (case.expected_season, case.expected_episode) {
        println!(
            "  target_episode=S{:02}E{:02} season_pack_fallback={}",
            season, episode, case.accept_season_pack
        );
    }

    for (index, evaluation) in result.evaluations.iter().take(5).enumerate() {
        let label = if evaluation.exact_episode {
            "exact"
        } else if evaluation.acceptable {
            "ok"
        } else if evaluation.conflicting {
            "conflict"
        } else {
            "miss"
        };

        println!("  {:>2}. {:<8} {}", index + 1, label, evaluation.filename);
        println!("      {}", evaluation.reasons.join(" | "));
    }

    println!();
}

fn print_summary(summary: &Summary) {
    println!("=== SUMMARY ===");
    println!("cases={}", summary.cases);
    println!("top1_pass={}/{}", summary.top1_pass, summary.cases);
    println!("topk_pass={}/{}", summary.topk_pass, summary.cases);
    println!("no_streams={}", summary.no_streams);
    println!(
        "episode_top1_exact={}/{}",
        summary.episode_top1_exact, summary.episode_cases
    );
    println!(
        "episode_topk_exact={}/{}",
        summary.episode_topk_exact, summary.episode_cases
    );
    println!("severe_false_positives={}", summary.severe_false_positives);
    println!("ci_failures={}", summary.ci_failures);
}
