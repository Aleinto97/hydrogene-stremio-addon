use hydrogene::scrapers::Scraper;
use hydrogene::scrapers::nyaa::NyaaScraper;
use hydrogene::scrapers::x1337::X1337Scraper;
use hydrogene::scrapers::rutor::RutorScraper;
use hydrogene::scrapers::rutracker::RuTrackerScraper;
use hydrogene::scrapers::bitsearch::BitsearchScraper;
use hydrogene::scrapers::yts::YtsScraper;
use hydrogene::scrapers::eztv::EztvScraper;
use hydrogene::scrapers::nekobt::NekoBtScraper;
use hydrogene::scrapers::tpb::TPBScraper;
use std::time::Duration;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    
    println!("========================================");
    println!("  PROVIDER DIAGNOSTIC TEST");
    println!("========================================\n");
    
    let test_queries = vec![
        ("Inception", "movie"),
        ("Breaking Bad", "series"),
        ("Attack on Titan", "series"),
    ];
    
    // Test each provider individually with detailed output
    let providers: Vec<Box<dyn Scraper>> = vec![
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
    
    for provider in providers {
        let name = provider.name();
        println!("\n═══════════════════════════════════════════════════════");
        println!("🔍 TESTING PROVIDER: {}", name);
        println!("═══════════════════════════════════════════════════════");
        
        for (query, content_type) in &test_queries {
            print!("\n  Query: '{}' ({})... ", query, content_type);
            
            match tokio::time::timeout(
                Duration::from_secs(15),
                provider.search(query, content_type)
            ).await {
                Ok(Ok(results)) => {
                    if results.is_empty() {
                        println!("❌ 0 results");
                    } else {
                        println!("✅ {} results", results.len());
                        // Show first 3 results
                        for (i, r) in results.iter().take(3).enumerate() {
                            let size_str = if r.size_gb >= 1.0 {
                                format!("{:.2} GB", r.size_gb)
                            } else {
                                format!("{:.0} MB", r.size_gb * 1024.0)
                            };
                            let quality = if r.title.to_uppercase().contains("2160P") || r.title.to_uppercase().contains("4K") {
                                "4K"
                            } else if r.title.to_uppercase().contains("1080P") {
                                "1080p"
                            } else {
                                "SD"
                            };
                            println!("    {}. {} [{} | {} | {} seeders]", 
                                i+1, 
                                &r.title[..r.title.len().min(50)],
                                quality,
                                size_str,
                                r.seeders
                            );
                        }
                        if results.len() > 3 {
                            println!("    ... and {} more", results.len() - 3);
                        }
                    }
                }
                Ok(Err(e)) => {
                    println!("❌ ERROR: {}", e);
                }
                Err(_) => {
                    println!("⏱️  TIMEOUT after 15s");
                }
            }
            
            // Small delay between queries
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
    
    println!("\n\n========================================");
    println!("  DIAGNOSTIC COMPLETE");
    println!("========================================");
    
    Ok(())
}
