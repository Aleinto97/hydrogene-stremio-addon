use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use hydrogene::calculate_match_score;
use hydrogene::matching::extract_year;
use hydrogene::metadata::MetadataClient;
use hydrogene::scrapers::{ScrapedTorrent, ScraperManager};
use reqwest::Client;

#[derive(Clone, Debug)]
struct Case {
    label: &'static str,
    content_type: &'static str,
    stremio_id: &'static str,
    expected_year: Option<u32>,
}

#[derive(Clone, Debug)]
struct RankedTorrent {
    torrent: ScrapedTorrent,
    score: i32,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    let http_client = Arc::new(
        Client::builder()
            .timeout(Duration::from_secs(20))
            .pool_max_idle_per_host(10)
            .build()?,
    );

    let metadata_client = Arc::new(MetadataClient::new(http_client)?);
    let scraper_manager = Arc::new(ScraperManager::new()?);

    let cases = vec![
        Case {
            label: "Avatar animated",
            content_type: "series",
            stremio_id: "tt0417299:1:1",
            expected_year: Some(2005),
        },
        Case {
            label: "Avatar live action",
            content_type: "series",
            stremio_id: "tt9018736:1:1",
            expected_year: Some(2024),
        },
        Case {
            label: "Death Note anime",
            content_type: "series",
            stremio_id: "tt0877057:1:1",
            expected_year: Some(2006),
        },
        Case {
            label: "The Crow 1994",
            content_type: "movie",
            stremio_id: "tt0109506",
            expected_year: Some(1994),
        },
        Case {
            label: "The Crow 2024",
            content_type: "movie",
            stremio_id: "tt1340094",
            expected_year: Some(2024),
        },
    ];

    println!("=== HOMONYM TEST WITH DIRECT STREMIO IDs ===\n");

    for case in cases {
        run_case(&metadata_client, &scraper_manager, &case).await?;
    }

    Ok(())
}

async fn run_case(
    metadata_client: &MetadataClient,
    scraper_manager: &ScraperManager,
    case: &Case,
) -> anyhow::Result<()> {
    let (target_season, target_episode) = parse_episode_target(case.stremio_id);
    let metadata = metadata_client
        .lookup_by_imdb(case.stremio_id, case.content_type)
        .await?;
    let target_year = metadata
        .year
        .as_deref()
        .and_then(|year| year.parse::<u32>().ok());

    let mut queries = metadata.search_queries.clone();
    queries.sort_by(|a, b| {
        score_search_query(b, &metadata.title, target_year, target_episode).cmp(
            &score_search_query(a, &metadata.title, target_year, target_episode),
        )
    });
    queries.dedup();
    queries.truncate(3);

    let mut all_torrents = Vec::new();
    let mut seen_hashes = HashSet::new();

    for query in &queries {
        let scraped = scraper_manager.scrape_all(query, case.content_type).await;
        for torrent in scraped {
            if seen_hashes.insert(torrent.info_hash.clone()) {
                all_torrents.push(torrent);
            }
        }
    }

    let mut ranked: Vec<RankedTorrent> = all_torrents
        .into_iter()
        .map(|torrent| RankedTorrent {
            score: calculate_match_score(
                &metadata.title,
                target_year,
                target_season,
                target_episode,
                &torrent.title,
                torrent.seeders,
                torrent.size_bytes,
            ),
            torrent,
        })
        .collect();

    ranked.sort_by(|a, b| b.score.cmp(&a.score));

    println!("CASE: {}", case.label);
    println!("ID: {}", case.stremio_id);
    println!("Resolved title: {}", metadata.title);
    println!("Resolved year: {:?}", target_year);
    println!("Queries used: {:?}", queries);

    if ranked.is_empty() {
        println!("No torrents found.\n");
        return Ok(());
    }

    for (index, item) in ranked.iter().take(8).enumerate() {
        let extracted_year = extract_year(&item.torrent.title);
        println!(
            "{:>2}. score={:<4} year={:<6?} source={:<14} {}",
            index + 1,
            item.score,
            extracted_year,
            item.torrent.source,
            item.torrent.title
        );
    }

    let top = &ranked[0];
    println!(
        "Top result year: {:?}, expected: {:?}\n",
        extract_year(&top.torrent.title),
        case.expected_year
    );

    Ok(())
}

fn parse_episode_target(stremio_id: &str) -> (Option<u32>, Option<u32>) {
    let parts: Vec<&str> = stremio_id.split(':').collect();
    if parts.len() >= 3 {
        (
            parts.get(1).and_then(|s| s.parse::<u32>().ok()),
            parts.get(2).and_then(|e| e.parse::<u32>().ok()),
        )
    } else {
        (None, None)
    }
}

fn score_search_query(
    query: &str,
    metadata_title: &str,
    target_year: Option<u32>,
    target_episode: Option<u32>,
) -> i32 {
    let query_lower = query.to_lowercase();
    let metadata_lower = metadata_title.to_lowercase();
    let mut score = 0;

    if query_lower == metadata_lower {
        score += 50;
    } else if query_lower.starts_with(&metadata_lower) {
        score += 25;
    }

    if let Some(year) = target_year {
        if query_lower.contains(&year.to_string()) {
            score += 20;
        }
    }

    if let Some(episode) = target_episode {
        let markers = [
            format!("e{:02}", episode),
            format!("ep{:02}", episode),
            format!(" {:02}", episode),
            format!(" {}", episode),
        ];
        if markers.iter().any(|marker| query_lower.contains(marker)) {
            score += 30;
        }
    }

    score - query.len() as i32 / 8
}
