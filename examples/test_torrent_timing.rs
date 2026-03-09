use hydrogene::scrapers::bitsearch::BitsearchScraper;
use hydrogene::scrapers::eztv::EztvScraper;
use hydrogene::scrapers::nekobt::NekoBtScraper;
use hydrogene::scrapers::nyaa::NyaaScraper;
use hydrogene::scrapers::rutor::RutorScraper;
use hydrogene::scrapers::rutracker::RuTrackerScraper;
use hydrogene::scrapers::tpb::TPBScraper;
use hydrogene::scrapers::x1337::X1337Scraper;
use hydrogene::scrapers::yts::YtsScraper;
use hydrogene::scrapers::{ScrapedTorrent, Scraper};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    println!("╔════════════════════════════════════════════════════════════════╗");
    println!("║     TEST RICERCA TORRENT - ANALISI TEMPI DI RISPOSTA          ║");
    println!("╚════════════════════════════════════════════════════════════════╝");

    // Test configuration
    let max_duration = Duration::from_secs(15);
    let test_queries = vec![
        ("Inception", "movie", "FILM"),
        ("Breaking Bad", "series", "SERIE TV"),
        ("Attack on Titan", "series", "ANIME"),
    ];

    for (query, content_type, label) in test_queries {
        println!("\n{}", "═".repeat(66));
        println!("  TEST: {} - '{}'", label, query);
        println!("{}", "═".repeat(66));

        let providers = get_providers_for_type(content_type)?;
        let mut total_results_timeline: Vec<(f64, usize)> = Vec::new();
        let start_time = Instant::now();

        // Create channels for each provider to report results
        let (tx, mut rx) = mpsc::unbounded_channel::<(String, Vec<ScrapedTorrent>, Instant)>();

        // Spawn provider searches
        let mut handles = Vec::new();
        for provider in providers {
            let tx = tx.clone();
            let provider_name = provider.name().to_string();
            let query = query.to_string();
            let content_type = content_type.to_string();

            let handle = tokio::spawn(async move {
                let provider_start = Instant::now();
                match provider.search(&query, &content_type).await {
                    Ok(results) => {
                        let elapsed = provider_start.elapsed().as_secs_f64();
                        let _ = tx.send((provider_name, results, Instant::now()));
                        elapsed
                    }
                    Err(_) => {
                        let _ = tx.send((provider_name, Vec::new(), Instant::now()));
                        provider_start.elapsed().as_secs_f64()
                    }
                }
            });
            handles.push(handle);
        }

        // Drop original sender
        drop(tx);

        // Collect results and track timeline
        let mut all_results: Vec<ScrapedTorrent> = Vec::new();
        let mut provider_times: HashMap<String, (f64, usize)> = HashMap::new();
        let mut last_second = 0u64;

        // Process results as they arrive
        while let Some((provider_name, results, _)) = rx.recv().await {
            let elapsed = start_time.elapsed().as_secs_f64();
            all_results.extend(results.clone());
            provider_times.insert(provider_name.clone(), (elapsed, results.len()));

            // Record results at each second interval
            let current_second = elapsed as u64;
            if current_second > last_second {
                for sec in last_second + 1..=current_second.min(max_duration.as_secs()) {
                    total_results_timeline.push((sec as f64, all_results.len()));
                }
                last_second = current_second;
            }

            println!(
                "  [{:>5.2}s] {}: {} torrent trovati",
                elapsed,
                provider_name,
                results.len()
            );
        }

        // Wait for max duration and fill remaining seconds
        let total_elapsed = start_time.elapsed();
        for sec in last_second + 1..=max_duration.as_secs() {
            if Duration::from_secs(sec) > total_elapsed {
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            total_results_timeline.push((sec as f64, all_results.len()));
        }

        // Print timeline summary
        println!("\n  📊 RIEPILOGO TEMPI:");
        println!("  {}", "─".repeat(60));

        let mut last_count = 0;
        for (time_sec, count) in total_results_timeline.iter().step_by(1) {
            let new_torrents = count.saturating_sub(last_count);
            let status = if new_torrents > 0 {
                format!("(+{} nuovi)", new_torrents)
            } else {
                "(nessun cambiamento)".to_string()
            };

            println!(
                "  {:>2}s: {:>3} torrent totali {}",
                *time_sec as usize, count, status
            );
            last_count = *count;
        }

        // Print provider details
        println!("\n  📈 DETTAGLIO PER PROVIDER:");
        println!("  {}", "─".repeat(60));
        let mut sorted_providers: Vec<_> = provider_times.iter().collect();
        sorted_providers.sort_by(|a, b| a.1 .0.partial_cmp(&b.1 .0).unwrap());

        for (name, (time, count)) in sorted_providers {
            let speed = if *time < 2.0 {
                "⚡ Veloce"
            } else if *time < 5.0 {
                "✅ Normale"
            } else if *time < 10.0 {
                "🐌 Lento"
            } else {
                "❌ Molto lento"
            };
            println!(
                "  {:<15} - {:>5.2}s - {:>3} torrent {}",
                name, time, count, speed
            );
        }

        // Summary stats
        let total_unique = all_results.len();
        let completed_in_1s = provider_times.values().filter(|(t, _)| *t <= 1.0).count();
        let completed_in_2s = provider_times.values().filter(|(t, _)| *t <= 2.0).count();
        let completed_in_5s = provider_times.values().filter(|(t, _)| *t <= 5.0).count();

        println!("\n  📋 STATISTICHE FINALI:");
        println!("  {}", "─".repeat(60));
        println!("  Totale torrent unici: {}", total_unique);
        println!(
            "  Provider che hanno risposto entro 1s: {}/{}",
            completed_in_1s,
            provider_times.len()
        );
        println!(
            "  Provider che hanno risposto entro 2s: {}/{}",
            completed_in_2s,
            provider_times.len()
        );
        println!(
            "  Provider che hanno risposto entro 5s: {}/{}",
            completed_in_5s,
            provider_times.len()
        );

        // Wait between tests
        tokio::time::sleep(Duration::from_secs(2)).await;
    }

    println!("\n{}", "═".repeat(66));
    println!("  TEST COMPLETATO");
    println!("{}", "═".repeat(66));

    Ok(())
}

fn get_providers_for_type(content_type: &str) -> anyhow::Result<Vec<Box<dyn Scraper>>> {
    let mut providers: Vec<Box<dyn Scraper>> = vec![
        Box::new(NyaaScraper::new()?),
        Box::new(X1337Scraper::new()?),
        Box::new(RutorScraper::new()?),
        Box::new(RuTrackerScraper::new()?),
        Box::new(BitsearchScraper::new()?),
        Box::new(YtsScraper::new()?),
        Box::new(EztvScraper::new()?),
        Box::new(NekoBtScraper::new()?),
        Box::new(TPBScraper::new()?),
    ];

    // Filter providers based on content type
    providers.retain(|p| match content_type {
        "movie" => p.supports_movies(),
        "series" => p.supports_series(),
        _ => true,
    });

    Ok(providers)
}
