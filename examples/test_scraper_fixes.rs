use hydrogene::scrapers::Scraper;
use std::time::Duration;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    
    println!("========================================");
    println!("  TEST SCRAPER MODIFICATI");
    println!("========================================\n");
    
    // Test Nyaa
    println!("\n1. Testing NyaaScraper...");
    let nyaa = hydrogene::scrapers::nyaa::NyaaScraper::new()?;
    match tokio::time::timeout(Duration::from_secs(10), nyaa.search("Attack on Titan", "series")).await {
        Ok(Ok(results)) => println!("   ✅ Nyaa: {} risultati", results.len()),
        Ok(Err(e)) => println!("   ❌ Nyaa error: {}", e),
        Err(_) => println!("   ⏱️  Nyaa timeout"),
    }
    
    // Test 1337x (con FlareSolverr)
    println!("\n2. Testing X1337Scraper (con FlareSolverr)...");
    let x1337 = hydrogene::scrapers::x1337::X1337Scraper::new()?;
    match tokio::time::timeout(Duration::from_secs(60), x1337.search("Inception", "movie")).await {
        Ok(Ok(results)) => println!("   ✅ 1337x: {} risultati", results.len()),
        Ok(Err(e)) => println!("   ❌ 1337x error: {}", e),
        Err(_) => println!("   ⏱️  1337x timeout"),
    }
    
    // Test Rutor
    println!("\n3. Testing RutorScraper...");
    let rutor = hydrogene::scrapers::rutor::RutorScraper::new()?;
    match tokio::time::timeout(Duration::from_secs(10), rutor.search("Inception", "movie")).await {
        Ok(Ok(results)) => {
            let with_seeders = results.iter().filter(|r| r.seeders > 0).count();
            println!("   ✅ Rutor: {} risultati ({} con seeders > 0)", results.len(), with_seeders);
            if let Some(first) = results.first() {
                println!("      Primo: '{}' - Seeders: {}", first.title.chars().take(50).collect::<String>(), first.seeders);
            }
        }
        Ok(Err(e)) => println!("   ❌ Rutor error: {}", e),
        Err(_) => println!("   ⏱️  Rutor timeout"),
    }
    
    // Test RuTracker (richiede cookie)
    println!("\n4. Testing RuTrackerScraper...");
    let rutracker = hydrogene::scrapers::rutracker::RuTrackerScraper::new()?;
    match tokio::time::timeout(Duration::from_secs(10), rutracker.search("Inception", "movie")).await {
        Ok(Ok(results)) => println!("   ✅ RuTracker: {} risultati (cookie: {})", results.len(), 
            if std::env::var("RUTRACKER_COOKIE").is_ok() { "presente" } else { "non configurato" }),
        Ok(Err(e)) => println!("   ❌ RuTracker error: {}", e),
        Err(_) => println!("   ⏱️  RuTracker timeout"),
    }
    
    // Test Nuovi Provider
    println!("\n5. Testing BitsearchScraper...");
    let bitsearch = hydrogene::scrapers::bitsearch::BitsearchScraper::new()?;
    match tokio::time::timeout(Duration::from_secs(10), bitsearch.search("Inception", "movie")).await {
        Ok(Ok(results)) => println!("   ✅ Bitsearch: {} risultati", results.len()),
        Ok(Err(e)) => println!("   ❌ Bitsearch error: {}", e),
        Err(_) => println!("   ⏱️  Bitsearch timeout"),
    }
    
    println!("\n6. Testing YtsScraper...");
    let yts = hydrogene::scrapers::yts::YtsScraper::new()?;
    match tokio::time::timeout(Duration::from_secs(10), yts.search("Inception", "movie")).await {
        Ok(Ok(results)) => println!("   ✅ YTS: {} risultati", results.len()),
        Ok(Err(e)) => println!("   ❌ YTS error: {}", e),
        Err(_) => println!("   ⏱️  YTS timeout"),
    }
    
    println!("\n7. Testing EztvScraper...");
    let eztv = hydrogene::scrapers::eztv::EztvScraper::new()?;
    match tokio::time::timeout(Duration::from_secs(10), eztv.search("Breaking Bad", "series")).await {
        Ok(Ok(results)) => println!("   ✅ EZTV: {} risultati", results.len()),
        Ok(Err(e)) => println!("   ❌ EZTV error: {}", e),
        Err(_) => println!("   ⏱️  EZTV timeout"),
    }
    
    // Test NekoBT
    println!("\n8. Testing NekoBtScraper...");
    let nekobt = hydrogene::scrapers::nekobt::NekoBtScraper::new()?;
    match tokio::time::timeout(Duration::from_secs(10), nekobt.search("Attack on Titan", "series")).await {
        Ok(Ok(results)) => println!("   ✅ NekoBT: {} risultati", results.len()),
        Ok(Err(e)) => println!("   ❌ NekoBT error: {}", e),
        Err(_) => println!("   ⏱️  NekoBT timeout"),
    }
    
    // Test TPB
    println!("\n9. Testing TPBScraper...");
    let tpb = hydrogene::scrapers::tpb::TPBScraper::new()?;
    match tokio::time::timeout(Duration::from_secs(10), tpb.search("Inception", "movie")).await {
        Ok(Ok(results)) => println!("   ✅ TPB: {} risultati", results.len()),
        Ok(Err(e)) => println!("   ❌ TPB error: {}", e),
        Err(_) => println!("   ⏱️  TPB timeout"),
    }
    
    println!("\n========================================");
    println!("  TEST COMPLETATO");
    println!("========================================");
    
    Ok(())
}
