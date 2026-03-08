use hydrogene::scrapers::{ScraperManager, ScrapedTorrent};
use hydrogene::debrid::RealDebridClient;
use reqwest::Client;
use std::sync::Arc;
use std::collections::HashMap;
use tracing::{info, warn, error};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    
    // Check for RD_API_KEY
    let rd_api_key = match std::env::var("RD_API_KEY") {
        Ok(key) => {
            info!("Real-Debrid API key found: {}", &key[..8.min(key.len())]);
            key
        }
        Err(_) => {
            error!("RD_API_KEY environment variable not set!");
            error!("Please set RD_API_KEY to test batch cache and resolve stream.");
            return Err(anyhow::anyhow!("RD_API_KEY not set"));
        }
    };

    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .init();
    
    println!("========================================");
    println!("  HYDROGEN CACHE & RESOLVE TEST");
    println!("  Tests: Batch Cache Check + Resolve Stream");
    println!("========================================\n");
    
    let http_client = Arc::new(Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .pool_max_idle_per_host(10)
        .build()?);
    
    let scraper_manager = Arc::new(ScraperManager::new()?);
    
    // Initialize Real-Debrid client
    let debrid_client = Arc::new(RealDebridClient::new()?);
    
    // Test case: Inception (popular movie likely to have cached torrents)
    let test_cases = vec![
        ("movie", "tt1375666", "Inception", ""),
    ];
    
    for (content_type, _id, title, _episode_info) in test_cases {
        println!("\n═══════════════════════════════════════════════════════");
        println!("📺 TESTING: {} ({})", title, content_type.to_uppercase());
        println!("═══════════════════════════════════════════════════════\n");
        
        // Use direct title for search
        let query = title.to_string();
        
        println!("🔍 Searching for torrents with query: '{}'...", query);
        let scraped = scraper_manager.scrape_all(&query, content_type).await;
        
        if scraped.is_empty() {
            println!("✗ No torrents found!");
            continue;
        }
        
        println!("✓ Found {} torrents\n", scraped.len());
        
        // Remove duplicates
        let mut seen_hashes = std::collections::HashSet::new();
        let mut unique_torrents: Vec<ScrapedTorrent> = scraped
            .into_iter()
            .filter(|t| seen_hashes.insert(t.info_hash.clone()))
            .collect();
        
        // Sort by seeders (most first)
        unique_torrents.sort_by(|a, b| b.seeders.cmp(&a.seeders));
        
        println!("📊 UNIQUE TORRENTS: {} (after deduplication)", unique_torrents.len());
        
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
        
        println!("🎬 FILTERED BY QUALITY (1080p/4K): {}", quality_torrents.len());
        
        if quality_torrents.is_empty() {
            println!("✗ No quality torrents found for cache check!");
            continue;
        }
        
        // Take top 10 for batch cache check (to avoid rate limiting)
        let torrents_to_check: Vec<ScrapedTorrent> = quality_torrents.into_iter().take(10).collect();
        
        println!("\n🔍 BATCH CACHE CHECK");
        println!("{}", "=".repeat(50));
        println!("Checking {} torrents on Real-Debrid...", torrents_to_check.len());
        
        // Perform batch cache check
        let start_time = std::time::Instant::now();
        let cached_torrents = match debrid_client.check_batch_cache(&torrents_to_check).await {
            Ok(torrents) => {
                let duration = start_time.elapsed();
                let cached_count = torrents.iter().filter(|t| t.is_cached).count();
                println!("✓ Batch cache check completed in {:.2?}", duration);
                println!("✓ Cached: {}/{} torrents", cached_count, torrents.len());
                
                // Show cached vs non-cached
                println!("\n📋 CACHE STATUS BREAKDOWN:");
                println!("{:<5} {:<12} {:<8} {:<50}", "#", "Status", "Seeders", "Title");
                println!("{}", "-".repeat(80));
                
                for (i, t) in torrents.iter().take(10).enumerate() {
                    let status = if t.is_cached { "✅ CACHED" } else { "❌ NOT CACHED" };
                    let title_short = t.title.chars().take(45).collect::<String>();
                    println!("{:<5} {:<12} {:<8} {}", 
                        i + 1, status, t.seeders, title_short);
                }
                
                torrents
            }
            Err(e) => {
                error!("✗ Batch cache check failed: {}", e);
                continue;
            }
        };
        
        // Find cached torrents for resolve test
        let cached: Vec<&ScrapedTorrent> = cached_torrents.iter()
            .filter(|t| t.is_cached)
            .collect();
        
        if cached.is_empty() {
            println!("\n✗ No cached torrents found to test resolve stream!");
            println!("This is normal if Real-Debrid doesn't have this content cached.");
            continue;
        }
        
        // Wait a few seconds after batch check to avoid rate limiting
        println!("\n⏳ Waiting 30 seconds to avoid rate limiting...");
        tokio::time::sleep(tokio::time::Duration::from_secs(30)).await;
        
        // Test resolve stream on first cached torrent
        let test_torrent = cached[0];
        println!("\n\n🎬 RESOLVE STREAM TEST");
        println!("{}", "=".repeat(50));
        println!("Testing resolution for:");
        println!("  Title: {}", test_torrent.title);
        println!("  Hash: {}", test_torrent.info_hash);
        println!("  Size: {:.2} GB", test_torrent.size_gb);
        println!("  Source: {}", test_torrent.source);
        println!("\nResolving... (this may take a few minutes for downloads)\n");
        
        let resolve_start = std::time::Instant::now();
        match debrid_client.resolve_magnet(&test_torrent.info_hash).await {
            Ok(stream_url) => {
                let resolve_duration = resolve_start.elapsed();
                println!("\n✅ RESOLVE SUCCESS!");
                println!("⏱️  Time taken: {:.2?}", resolve_duration);
                println!("\n🔗 Stream URL:");
                println!("{}", stream_url);
                println!("\n📊 URL Info:");
                println!("  Length: {} characters", stream_url.len());
                if stream_url.starts_with("http") {
                    println!("  Protocol: HTTP/HTTPS ✓");
                }
                
                // Verify URL is accessible (optional, just HEAD request)
                println!("\n🔍 Verifying URL accessibility...");
                match http_client.head(&stream_url).send().await {
                    Ok(response) => {
                        println!("✓ URL is reachable: HTTP {}", response.status());
                        if let Some(content_length) = response.headers().get("content-length") {
                            println!("✓ Content-Length: {:?}", content_length);
                        }
                    }
                    Err(e) => {
                        warn!("⚠ Could not verify URL: {}", e);
                    }
                }
            }
            Err(e) => {
                error!("\n❌ RESOLVE FAILED: {}", e);
                error!("The torrent may need to be downloaded first.");
            }
        }
        
        // Summary
        println!("\n\n📊 TEST SUMMARY");
        println!("{}", "=".repeat(50));
        println!("Batch Cache Check: ✅ WORKING");
        println!("  - Checked: {} torrents", torrents_to_check.len());
        println!("  - Cached: {} torrents", cached.len());
        
        let resolve_status = if cached.is_empty() { 
            "⏭️ SKIPPED (no cached torrents)" 
        } else { 
            "✅ TESTED" 
        };
        println!("Resolve Stream: {}", resolve_status);
    }
    
    println!("\n\n✅ All tests completed!");
    
    Ok(())
}
