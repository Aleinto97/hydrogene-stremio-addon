use hydrogene::metadata::MetadataClient;
use hydrogene::scrapers::{ScrapedTorrent, Scraper, ScraperManager};
use reqwest::Client;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{error, info, warn};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    // Initialize tracing
    tracing_subscriber::fmt().with_env_filter("info").init();

    println!("========================================");
    println!("  HYDROGEN LOCAL TEST - NO DATABASE");
    println!("========================================\n");

    let http_client = Arc::new(
        Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .pool_max_idle_per_host(10)
            .build()?,
    );

    let scraper_manager = Arc::new(ScraperManager::new()?);
    let metadata_client = Arc::new(MetadataClient::new(http_client.clone())?);

    // Test cases
    let test_cases = vec![
        ("anime", "anilist:16498:1", "Attack on Titan", "S01E01"), // AniList ID with episode
        ("series", "tt0903747:1:1", "Breaking Bad", "S01E01"),     // IMDB ID with season:episode
        ("movie", "tt1375666", "Inception", ""),                   // IMDB ID for movie
    ];

    let mut total_results = HashMap::new();
    let mut provider_status: HashMap<String, (usize, bool)> = HashMap::new();

    for (content_type, id, title, episode_info) in test_cases {
        println!("\n═══════════════════════════════════════════════════════");
        println!("📺 TESTING: {} ({})", title, content_type.to_uppercase());
        if !episode_info.is_empty() {
            println!("   Episode: {}", episode_info);
        }
        println!("   ID: {}", id);
        println!("═══════════════════════════════════════════════════════\n");

        // Get metadata
        let search_queries = if id.starts_with("anilist:") {
            // Anime: resolve via AniList
            let parts: Vec<&str> = id.split(':').collect();
            let anime_id = parts.get(1).unwrap_or(&"");
            let episode = parts.get(2).and_then(|e| e.parse::<u32>().ok());

            match metadata_client
                .resolve_anime_title(&format!("anilist:{}", anime_id))
                .await
            {
                Some(anime_title) => {
                    let mut queries = vec![anime_title.clone()];
                    if let Some(ep) = episode {
                        queries.push(format!("{} {:02}", anime_title, ep));
                        queries.push(format!("{} E{:02}", anime_title, ep));
                    }
                    println!("✓ Resolved anime title: {}", anime_title);
                    queries
                }
                None => {
                    println!("✗ Failed to resolve anime title, using raw ID");
                    vec![anime_id.to_string()]
                }
            }
        } else if id.starts_with("tt") {
            // Movie/Series: use TMDB or direct title
            if let (Some(season), Some(episode)) = if id.contains(':') {
                let parts: Vec<&str> = id.split(':').collect();
                (
                    parts.get(1).and_then(|s| s.parse::<u32>().ok()),
                    parts.get(2).and_then(|e| e.parse::<u32>().ok()),
                )
            } else {
                (None, None)
            } {
                // Series episode
                vec![
                    format!("{} S{:02}E{:02}", title, season, episode),
                    format!("{} S{}E{}", title, season, episode),
                    title.to_string(),
                ]
            } else {
                // Movie
                vec![title.to_string()]
            }
        } else {
            vec![id.to_string()]
        };

        println!("🔍 Search queries: {:?}\n", search_queries);

        // Scrape for each query until we get results
        let mut all_torrents = Vec::new();
        let mut _query_used = String::new();

        for query in &search_queries {
            println!("  Searching with query: '{}'...", query);
            let scraped = scraper_manager.scrape_all(query, content_type).await;

            if !scraped.is_empty() {
                println!("  ✓ Found {} torrents\n", scraped.len());
                all_torrents = scraped;
                _query_used = query.clone();
                break;
            } else {
                println!("  ✗ No results\n");
            }

            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        }

        // Remove duplicates
        let mut seen_hashes = std::collections::HashSet::new();
        let mut unique_torrents: Vec<ScrapedTorrent> = all_torrents
            .into_iter()
            .filter(|t| seen_hashes.insert(t.info_hash.clone()))
            .collect();

        // Sort by seeders (most first)
        unique_torrents.sort_by(|a, b| b.seeders.cmp(&a.seeders));

        println!(
            "📊 UNIQUE TORRENTS: {} (after deduplication)",
            unique_torrents.len()
        );

        // Filter by quality (only 1080p, 2160p, 4K)
        let quality_torrents: Vec<ScrapedTorrent> = unique_torrents
            .into_iter()
            .filter(|t| {
                let title_upper = t.title.to_uppercase();
                let has_1080p = title_upper.contains("1080P");
                let has_2160p = title_upper.contains("2160P");
                let has_4k = title_upper.contains("4K") || title_upper.contains("UHD");
                has_1080p || has_2160p || has_4k
            })
            .collect();

        println!(
            "🎬 FILTERED BY QUALITY (1080p/4K): {}",
            quality_torrents.len()
        );

        // Separate by quality and sort
        let mut results_4k: Vec<ScrapedTorrent> = quality_torrents
            .iter()
            .filter(|t| {
                let title_upper = t.title.to_uppercase();
                title_upper.contains("2160P")
                    || title_upper.contains("4K")
                    || title_upper.contains("UHD")
            })
            .cloned()
            .collect();

        let mut results_1080p: Vec<ScrapedTorrent> = quality_torrents
            .into_iter()
            .filter(|t| {
                let title_upper = t.title.to_uppercase();
                title_upper.contains("1080P")
                    && !title_upper.contains("2160P")
                    && !title_upper.contains("4K")
                    && !title_upper.contains("UHD")
            })
            .collect();

        // Sort by size (largest first)
        results_4k.sort_by(|a, b| b.size_bytes.cmp(&a.size_bytes));
        results_1080p.sort_by(|a, b| b.size_bytes.cmp(&a.size_bytes));

        // Store total for this test
        let total_count = results_4k.len() + results_1080p.len();
        total_results.insert(
            title.to_string(),
            (total_count, results_4k.len(), results_1080p.len()),
        );

        // Display results by quality
        if !results_4k.is_empty() {
            println!("\n🏆 4K / 2160p RESULTS ({} found):", results_4k.len());
            println!(
                "{:<5} {:<60} {:<12} {:<15} {}",
                "#", "Title", "Size", "Seeders", "Source"
            );
            println!("{}", "-".repeat(110));

            for (i, t) in results_4k.iter().take(10).enumerate() {
                let size_str = if t.size_gb >= 1.0 {
                    format!("{:.2} GB", t.size_gb)
                } else {
                    format!("{:.0} MB", t.size_gb * 1024.0)
                };
                println!(
                    "{:<5} {:<60} {:<12} {:<15} {}",
                    i + 1,
                    t.title.chars().take(60).collect::<String>(),
                    size_str,
                    t.seeders,
                    t.source
                );
            }
            if results_4k.len() > 10 {
                println!("... and {} more", results_4k.len() - 10);
            }
        } else {
            println!("\n🏆 4K / 2160p RESULTS: None found");
        }

        if !results_1080p.is_empty() {
            println!("\n📺 1080p RESULTS ({} found):", results_1080p.len());
            println!(
                "{:<5} {:<60} {:<12} {:<15} {}",
                "#", "Title", "Size", "Seeders", "Source"
            );
            println!("{}", "-".repeat(110));

            for (i, t) in results_1080p.iter().take(10).enumerate() {
                let size_str = if t.size_gb >= 1.0 {
                    format!("{:.2} GB", t.size_gb)
                } else {
                    format!("{:.0} MB", t.size_gb * 1024.0)
                };
                println!(
                    "{:<5} {:<60} {:<12} {:<15} {}",
                    i + 1,
                    t.title.chars().take(60).collect::<String>(),
                    size_str,
                    t.seeders,
                    t.source
                );
            }
            if results_1080p.len() > 10 {
                println!("... and {} more", results_1080p.len() - 10);
            }
        } else {
            println!("\n📺 1080p RESULTS: None found");
        }

        // Track provider status
        for t in results_4k.iter().chain(results_1080p.iter()) {
            let entry = provider_status.entry(t.source.clone()).or_insert((0, true));
            entry.0 += 1;
        }
    }

    // Summary
    println!("\n\n");
    println!("╔════════════════════════════════════════════════════════════════════╗");
    println!("║                         TEST SUMMARY                               ║");
    println!("╚════════════════════════════════════════════════════════════════════╝");
    println!();

    println!("📈 RESULTS BY CONTENT:");
    println!("{:<30} {:<12} {:<12} {}", "Title", "Total", "4K", "1080p");
    println!("{}", "-".repeat(70));
    for (title, (total, count_4k, count_1080p)) in &total_results {
        println!(
            "{:<30} {:<12} {:<12} {}",
            title, total, count_4k, count_1080p
        );
    }

    println!("\n📊 PROVIDER STATUS:");
    println!("{:<20} {:<12} {}", "Provider", "Results", "Status");
    println!("{}", "-".repeat(50));
    let providers = vec!["Nyaa", "TPB", "1337x", "Rutor", "RuTracker", "WatchSoMuch"];
    for provider in providers {
        if let Some((count, working)) = provider_status.get(provider) {
            let status = if *working {
                "✅ WORKING"
            } else {
                "❌ FAILED"
            };
            println!("{:<20} {:<12} {}", provider, count, status);
        } else {
            println!("{:<20} {:<12} {}", provider, 0, "❌ NO RESULTS");
        }
    }

    let grand_total: usize = total_results.values().map(|(t, _, _)| t).sum();
    println!("\n🎯 TOTAL RESULTS ACROSS ALL TESTS: {}", grand_total);
    println!("\n✅ Test completed without database (local mode)");

    Ok(())
}
