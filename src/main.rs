use axum::{
    routing::{get, post},
    Router,
    extract::{Path, State, Request},
    response::{Json, Redirect, Response},
    middleware,
};
use std::net::SocketAddr;
use std::sync::Arc;
use tracing::{info, error};

mod db;
mod debrid;
mod metadata;
mod scrapers;

use db::DbPool;
use metadata::MetadataClient;
use scrapers::{ScraperManager, ScrapedTorrent};

#[derive(Clone)]
struct AppState {
    db_pool: DbPool,
    scraper_manager: Arc<ScraperManager>,
    debrid_client: Arc<debrid::RealDebridClient>,
    metadata_client: Arc<MetadataClient>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load environment variables
    dotenvy::dotenv().ok();
    
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    info!("Starting Stremio Addon Server...");

    // Initialize database pool
    let db_pool = db::init_pool().await?;
    info!("Database pool initialized");

    // Initialize shared HTTP client for connection pooling
    let http_client = Arc::new(reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .pool_max_idle_per_host(10)
        .build()?);
    info!("HTTP client initialized with connection pooling");

    // Initialize scraper manager
    let scraper_manager = Arc::new(ScraperManager::new()?);
    
    // Initialize Real-Debrid client
    let debrid_client = Arc::new(debrid::RealDebridClient::new()?);

    // Initialize metadata client with shared HTTP client
    let metadata_client = Arc::new(MetadataClient::new(http_client.clone())?);

    let app_state = AppState {
        db_pool,
        scraper_manager,
        debrid_client,
        metadata_client,
    };

    // Build router
    let app = Router::new()
        .route("/", get(root_handler))
        .route("/manifest.json", get(manifest_handler))
        .route("/stream/:type/:id.json", get(stream_handler))
        .route("/resolve/:hash", get(resolve_handler))
        .layer(tower_http::cors::CorsLayer::permissive())
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .with_state(app_state);

    // Add global timeout middleware (15 seconds max per request)
    let app = app.layer(axum::middleware::from_fn(timeout_middleware));
    
    // Get port from env or default to 8080
    let port = std::env::var("PORT")
        .unwrap_or_else(|_| "8080".to_string())
        .parse::<u16>()?;
    
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    info!("Server listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

// Timeout middleware - returns empty result if request takes too long
async fn timeout_middleware(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let uri = req.uri().to_string();
    
    // Skip timeout for health checks, manifest, and resolve endpoint
    if uri == "/" || uri == "/manifest.json" || uri.starts_with("/resolve/") {
        return next.run(req).await;
    }
    
    // Apply 30 second timeout for stream requests (15s was too short for anime with Kitsu lookup + scraping)
    match tokio::time::timeout(
        std::time::Duration::from_secs(30),
        next.run(req)
    ).await {
        Ok(response) => response,
        Err(_) => {
            tracing::warn!("Request timeout for {} after 30s", uri);
            // Return empty streams on timeout
            let body = axum::body::Body::from(r#"{"streams": []}"#);
            axum::response::Response::builder()
                .status(200)
                .header("Content-Type", "application/json")
                .body(body)
                .unwrap()
        }
    }
}

async fn root_handler() -> &'static str {
    "Hydrogen Stremio Addon - Use /manifest.json for Stremio"
}

#[derive(serde::Serialize)]
struct Manifest {
    id: String,
    version: String,
    name: String,
    description: String,
    types: Vec<String>,
    catalogs: Vec<serde_json::Value>,
    resources: Vec<String>,
    id_prefixes: Vec<String>,
    behavior_hints: serde_json::Value,
}

async fn manifest_handler() -> Json<Manifest> {
    let addon_name = std::env::var("ADDON_NAME")
        .unwrap_or_else(|_| "Hydrogen Torrents".to_string());
    let addon_desc = std::env::var("ADDON_DESCRIPTION")
        .unwrap_or_else(|_| "High-performance torrent scraper".to_string());
    let addon_id = std::env::var("ADDON_ID")
        .unwrap_or_else(|_| "ai.hydrogen.stremio".to_string());
    let addon_version = std::env::var("ADDON_VERSION")
        .unwrap_or_else(|_| "0.1.0".to_string());

    Json(Manifest {
        id: addon_id,
        version: addon_version,
        name: addon_name,
        description: addon_desc,
        types: vec!["movie".to_string(), "series".to_string()],
        catalogs: vec![],
        resources: vec!["stream".to_string()],
        id_prefixes: vec!["tt".to_string(), "kitsu".to_string()],
        behavior_hints: serde_json::json!({
            "configurable": false,
            "configurationRequired": false
        }),
    })
}

#[derive(serde::Serialize)]
struct StreamResponse {
    streams: Vec<Stream>,
}

#[derive(serde::Serialize)]
struct Stream {
    name: String,
    description: String,
    #[serde(rename = "infoHash")]
    info_hash: Option<String>,
    #[serde(rename = "url")]
    url: Option<String>,
    #[serde(rename = "behaviorHints")]
    behavior_hints: serde_json::Value,
}

// Extract release group from torrent title
// Handles patterns like [Group], -Group, .Group, or Group at end
fn extract_release_group(title: &str) -> String {
    let title_upper = title.to_uppercase();
    
    // Try bracket pattern first: [Group]
    if let Some(start) = title.find('[') {
        if let Some(end) = title[start..].find(']') {
            let group = &title[start+1..start+end];
            if !group.is_empty() && group.len() < 30 {
                return group.to_string();
            }
        }
    }
    
    // Try dash pattern: -Group or _Group
    if let Some(dash_pos) = title.rfind('-') {
        if dash_pos > 0 && dash_pos < title.len() - 1 {
            let after_dash = &title[dash_pos+1..];
            // Check if there's another dash, take the last part
            if let Some(second_dash) = after_dash.rfind('-') {
                let group = &after_dash[second_dash+1..];
                if !group.is_empty() && group.len() < 30 && !group.contains('.') {
                    return group.trim().to_string();
                }
            }
            let group = after_dash.trim();
            if !group.is_empty() && group.len() < 30 && !group.contains('.') {
                return group.to_string();
            }
        }
    }
    
    // Try underscore pattern: _Group
    if let Some(underscore_pos) = title.rfind('_') {
        if underscore_pos > 0 && underscore_pos < title.len() - 1 {
            let group = &title[underscore_pos+1..];
            if !group.is_empty() && group.len() < 30 && !group.contains('.') {
                return group.trim().to_string();
            }
        }
    }
    
    // Try period pattern for last segment: .Group
    let parts: Vec<&str> = title.split('.').collect();
    if parts.len() > 1 {
        let last = parts.last().unwrap();
        // Check if last part looks like a release group (not a file extension)
        let extensions = ["MKV", "MP4", "AVI", "TS", "WMV", "MOV", "FLV", "WEBM"];
        if !extensions.contains(&last.to_uppercase().as_str()) && 
           last.len() > 1 && last.len() < 30 {
            return last.to_string();
        }
    }
    
    // Default fallback - try to get last word
    title.split_whitespace()
        .last()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && s.len() < 30)
        .unwrap_or_else(|| "Unknown".to_string())
}

// Parse title to extract season and episode information
// Supports formats like: "Attack on Titan S01E01", "Show - Season 1 Episode 2", "Anime - 01"
fn parse_title_for_episode(title: &str) -> (String, Option<u32>, Option<u32>) {
    let title_upper = title.to_uppercase();
    
    // Pattern 1: S01E01, S1E1, Season 1 Episode 1
    let season_episode_regex = regex::Regex::new(r"S(\d{1,2})E(\d{1,3})").unwrap();
    if let Some(caps) = season_episode_regex.captures(&title_upper) {
        let season = caps.get(1).and_then(|m| m.as_str().parse().ok());
        let episode = caps.get(2).and_then(|m| m.as_str().parse().ok());
        let clean_title = season_episode_regex.replace(title, "").to_string();
        return (clean_title.trim().to_string(), season, episode);
    }
    
    // Pattern 2: Season X Episode Y
    let season_text_regex = regex::Regex::new(r"SEASON\s+(\d+).*?EPISODE\s+(\d+)").unwrap();
    if let Some(caps) = season_text_regex.captures(&title_upper) {
        let season = caps.get(1).and_then(|m| m.as_str().parse().ok());
        let episode = caps.get(2).and_then(|m| m.as_str().parse().ok());
        let clean_title = season_text_regex.replace(title, "").to_string();
        return (clean_title.trim().to_string(), season, episode);
    }
    
    // Pattern 3: Just Episode number at end (common for anime)
    // "Attack on Titan - 01" or "Anime Episode 05"
    let ep_only_regex = regex::Regex::new(r"(?:EPISODE\s+|E|EP|\s-\s)(\d{1,3})$").unwrap();
    if let Some(caps) = ep_only_regex.captures(&title_upper) {
        let episode = caps.get(1).and_then(|m| m.as_str().parse().ok());
        let clean_title = ep_only_regex.replace(title, "").to_string();
        return (clean_title.trim().to_string(), None, episode);
    }
    
    // Pattern 4: Just season
    let season_only_regex = regex::Regex::new(r"S(?:EASON\s+)?(\d{1,2})").unwrap();
    if let Some(caps) = season_only_regex.captures(&title_upper) {
        let season = caps.get(1).and_then(|m| m.as_str().parse().ok());
        let clean_title = season_only_regex.replace(title, "").to_string();
        return (clean_title.trim().to_string(), season, None);
    }
    
    // No patterns matched, return original title
    (title.to_string(), None, None)
}

async fn stream_handler(
    State(state): State<AppState>,
    Path((content_type, id)): Path<(String, String)>,
) -> Json<StreamResponse> {
    // Strip .json extension if present
    let id = id.trim_end_matches(".json").to_string();
    
    // Stremio sends IDs in various formats:
    // - Movies: tt1375666
    // - Series episodes: tt0903747:1:2 (ID:season:episode)
    // - Anime: kitsu:12345 or anilist:16498
    // Extract base ID for metadata lookup
    let base_id = id.split(':').next().unwrap_or(&id).to_string();
    let metadata_id = if id.contains(':') && !id.starts_with("kitsu:") && !id.starts_with("anilist:") {
        // For series episodes (tt:season:episode format), use base ID (tt0903747)
        base_id.clone()
    } else {
        // For movies and anime (kitsu: or anilist:), use full ID
        id.clone()
    };
    
    eprintln!("DEBUG: Stream request: type={}, id={}, base_id={}, metadata_id={}", 
              content_type, id, base_id, metadata_id);
    info!("Stream request: type={}, id={}", content_type, id);

    // Try to get from cache first
    let cached = match db::get_cached_torrents(&state.db_pool, &id).await {
        Ok(t) => {
            eprintln!("DEBUG: Cache query successful, {} torrents", t.len());
            Some(t)
        }
        Err(e) => {
            eprintln!("DEBUG: Cache query failed: {}", e);
            None
        }
    };
    
    let torrents = if let Some(cached_torrents) = cached {
        if !cached_torrents.is_empty() {
            eprintln!("DEBUG: Cache hit for {} with {} torrents", id, cached_torrents.len());
            info!("Cache hit for {}", id);
            cached_torrents
        } else {
            eprintln!("DEBUG: Cache empty for {}, will scrape fresh", id);
            // Continue to scraping section below
            Vec::new() // placeholder, will be overwritten
        }
    } else {
        eprintln!("DEBUG: Cache miss for {}, looking up metadata...", id);
        info!("Cache miss for {}, looking up metadata...", id);
        Vec::new() // placeholder, will be overwritten
    };
    
    // If torrents is still empty, we need to scrape
    eprintln!("STEP 1: Checking if torrents is empty. Current count: {}", torrents.len());
    let torrents = if torrents.is_empty() {
        eprintln!("STEP 2: Entering scraping block - metadata_id={}", metadata_id);
        
        // Determine search queries
        let search_queries: Vec<String> = if metadata_id.starts_with("kitsu:") || metadata_id.starts_with("anilist:") {
            // --- ANIME: Use CDN/GraphQL to resolve title ---
            eprintln!("STEP 3: ANIME BRANCH - metadata_id is anime: {}", metadata_id);
            
            // Extract episode number from ID if present (format: kitsu:ID:EP or anilist:ID:EP)
            let parts: Vec<&str> = metadata_id.split(':').collect();
            eprintln!("STEP 4: ID parts: {:?}", parts);
            
            let episode = if parts.len() >= 3 {
                let ep = parts[2].parse::<u32>().ok();
                eprintln!("STEP 5: Parsed episode: {:?}", ep);
                ep
            } else {
                eprintln!("STEP 5: No episode in ID");
                None
            };
            
            // Resolve anime title using CDN/GraphQL
            eprintln!("STEP 6: Calling resolve_anime_title() for {}", metadata_id);
            match state.metadata_client.resolve_anime_title(&metadata_id).await {
                Some(title) => {
                    eprintln!("STEP 7: SUCCESS - Resolved to title: '{}'", title);
                    
                    // Build queries with episode info
                    let mut queries = vec![title.clone()];
                    
                    if let Some(ep) = episode {
                        // Add episode-specific queries
                        queries.push(format!("{} {:02}", title, ep));  // "Attack on Titan 01"
                        queries.push(format!("{} E{:02}", title, ep)); // "Attack on Titan E01"
                        eprintln!("STEP 8: Added episode {} queries. Total: {}", ep, queries.len());
                    } else {
                        eprintln!("STEP 8: No episode, using title only: {}", queries.len());
                    }
                    
                    queries
                }
                None => {
                    eprintln!("STEP 7: FAILED - resolve_anime_title() returned None");
                    vec![metadata_id.clone()]
                }
            }
        } else if metadata_id.starts_with("tt") {
            // --- MOVIES/SERIES: Use TMDB ---
            eprintln!("DEBUG: IMDB ID detected: {}", metadata_id);
            
            match state.metadata_client.lookup_by_imdb(&metadata_id, &content_type).await {
                Ok(meta) => {
                    eprintln!("DEBUG: Found metadata: {} ({} queries)", meta.title, meta.search_queries.len());
                    meta.search_queries
                }
                Err(e) => {
                    eprintln!("DEBUG: Failed to lookup metadata: {}", e);
                    vec![metadata_id.clone()]
                }
            }
        } else {
            // --- DIRECT TITLE SEARCH ---
            eprintln!("DEBUG: Direct title search: {}", metadata_id);
            vec![metadata_id.clone()]
        };

        // Try scraping with different search queries
        let mut all_torrents = Vec::new();
        
        eprintln!("DEBUG: Will try {} queries", search_queries.len());
        for query in &search_queries {
            eprintln!("DEBUG: Scraping for query: {}", query);
            info!("Scraping for query: {}", query);
            let scraped = state.scraper_manager.scrape_all(query, &content_type).await;
            eprintln!("DEBUG: Scraped {} torrents for query {}", scraped.len(), query);
            all_torrents.extend(scraped);
            
            // If we got results, stop trying other queries
            if !all_torrents.is_empty() {
                break;
            }
            
            // Small delay between queries to avoid rate limiting
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }
        
        // Remove duplicates and sort
        use std::collections::HashSet;
        let mut seen_hashes = HashSet::new();
        let mut unique: Vec<ScrapedTorrent> = all_torrents
            .into_iter()
            .filter(|t| seen_hashes.insert(t.info_hash.clone()))
            .collect();
        
        unique.sort_by(|a, b| b.seeders.cmp(&a.seeders));
        unique.truncate(50); // Keep top 50
        
        // Save to cache
        if let Err(e) = db::cache_torrents(&state.db_pool, &id, &unique).await {
            eprintln!("DEBUG: Failed to cache torrents: {}", e);
            error!("Failed to cache torrents: {}", e);
        }
        
        eprintln!("DEBUG: Found {} unique torrents for {}", unique.len(), id);
        info!("Found {} unique torrents for {}", unique.len(), id);
        unique
    } else {
        // If we have cached torrents, use them
        eprintln!("DEBUG: Using {} cached torrents", torrents.len());
        torrents
    };

    // Convert to Stremio streams
    // Get base URL for absolute URLs
    let base_url = std::env::var("BASE_URL")
        .unwrap_or_else(|_| "http://torrentio-stack-aleinto97-54335f00.koyeb.app".to_string());
    
    // Parse the ID to extract season and episode for series
    let (target_season, target_episode) = if id.contains(':') && !id.starts_with("kitsu:") {
        let parts: Vec<&str> = id.split(':').collect();
        if parts.len() >= 3 {
            let season = parts[1].parse::<u32>().ok();
            let episode = parts[2].parse::<u32>().ok();
            (season, episode)
        } else {
            (None, None)
        }
    } else if id.starts_with("kitsu:") {
        // For anime, extract from kitsu ID - but we don't have season/ep info
        (None, None)
    } else {
        (None, None)
    };

    // Filter torrents by quality (only 1080p, 2160p, 4K, UHD), minimum seeders, and season/episode matching
    let min_seeders: i32 = std::env::var("MIN_SEEDERS")
        .unwrap_or_else(|_| "1".to_string())
        .parse()
        .unwrap_or(1);
    
    let filtered_torrents: Vec<ScrapedTorrent> = torrents
        .into_iter()
        .filter(|t| t.seeders >= min_seeders) // Filter out dead torrents
        .filter(|t| {
            let title_upper = t.title.to_uppercase();
            // Only allow 1080p, 2160p, 4K, UHD
            let has_1080p = title_upper.contains("1080P");
            let has_2160p = title_upper.contains("2160P");
            let has_4k = title_upper.contains("4K") || title_upper.contains("UHD");
            has_1080p || has_2160p || has_4k
        })
        .filter(|t| {
            // Filter by season/episode if we have target info
            if let (Some(target_season), Some(target_episode)) = (target_season, target_episode) {
                let title_upper = t.title.to_uppercase();
                
                // Check for exact season/episode match
                let s_pattern = format!("S{:02}E{:02}", target_season, target_episode);
                let s_pattern_alt = format!("S{}E{}", target_season, target_episode);
                let s_pattern_alt2 = format!("S{:02}E{}", target_season, target_episode);
                let s_pattern_alt3 = format!("S{}E{:02}", target_season, target_episode);
                
                // Also check for season-only torrents (full season packs should be excluded for specific episode requests)
                let season_only_pattern = format!("S{:02}", target_season);
                let season_only_pattern_alt = format!("SEASON {}", target_season);
                
                let has_exact_episode = title_upper.contains(&s_pattern) ||
                                        title_upper.contains(&s_pattern_alt) ||
                                        title_upper.contains(&s_pattern_alt2) ||
                                        title_upper.contains(&s_pattern_alt3);
                
                // Exclude season packs when looking for specific episode
                let is_season_pack = (title_upper.contains(&season_only_pattern) || 
                                     title_upper.contains(&season_only_pattern_alt)) &&
                                     !has_exact_episode;
                
                has_exact_episode && !is_season_pack
            } else {
                true // No specific season/episode, include all
            }
        })
        .collect();

    // Sort by quality first (4K > 1080p), then by seeders (most seeders first)
    let mut sorted_torrents = filtered_torrents;
    sorted_torrents.sort_by(|a, b| {
        // First by quality priority (2160p/4K > 1080p)
        let a_title = a.title.to_uppercase();
        let b_title = b.title.to_uppercase();
        let a_is_4k = a_title.contains("2160P") || a_title.contains("4K") || a_title.contains("UHD");
        let b_is_4k = b_title.contains("2160P") || b_title.contains("4K") || b_title.contains("UHD");
        
        match (a_is_4k, b_is_4k) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => b.seeders.cmp(&a.seeders) // Then by seeders (most first)
        }
    });

    // Keep top 20 results
    sorted_torrents.truncate(20);

    let streams: Vec<Stream> = sorted_torrents
        .into_iter()
        .map(|t| {
            let title_upper = t.title.to_uppercase();
            
            // Extract resolution with quality icons
            let resolution = if title_upper.contains("2160P") || title_upper.contains("4K") || title_upper.contains("UHD") {
                "4K"
            } else if title_upper.contains("1080P") {
                "1080p"
            } else if title_upper.contains("720P") {
                "720p"
            } else if title_upper.contains("480P") {
                "480p"
            } else {
                "SD"
            };
            
            // Extract HDR/Dolby Vision info
            let mut hdr_info = Vec::new();
            if title_upper.contains("DV") || title_upper.contains("DOBY VISION") || title_upper.contains("DOVI") {
                hdr_info.push("DV");
            }
            if title_upper.contains("HDR10+") || title_upper.contains("HDR10PLUS") {
                hdr_info.push("HDR10+");
            } else if title_upper.contains("HDR10") {
                hdr_info.push("HDR10");
            } else if title_upper.contains("HDR") {
                hdr_info.push("HDR");
            }
            if title_upper.contains("HLG") {
                hdr_info.push("HLG");
            }
            
            // Extract codec/encode format
            let mut codec = String::new();
            if title_upper.contains("X265") || title_upper.contains("HEVC") || title_upper.contains("H.265") || title_upper.contains("H265") {
                codec = "x265".to_string();
            } else if title_upper.contains("X264") || title_upper.contains("AVC") || title_upper.contains("H.264") || title_upper.contains("H264") {
                codec = "x264".to_string();
            } else if title_upper.contains("AV1") {
                codec = "AV1".to_string();
            } else if title_upper.contains("VP9") {
                codec = "VP9".to_string();
            }
            
            // Extract source type
            let mut source = String::new();
            if title_upper.contains("WEB-DL") || title_upper.contains("WEBDL") {
                source = "WEB-DL".to_string();
            } else if title_upper.contains("WEBRIP") || title_upper.contains("WEB-RIP") {
                source = "WEB-Rip".to_string();
            } else if title_upper.contains("BLURAY") || title_upper.contains("BLU-RAY") {
                source = "BluRay".to_string();
            } else if title_upper.contains("BDRIP") || title_upper.contains("BD-RIP") {
                source = "BDRip".to_string();
            } else if title_upper.contains("HDTV") {
                source = "HDTV".to_string();
            } else if title_upper.contains("HDCAM") || title_upper.contains("HD-CAM") {
                source = "HDCAM".to_string();
            } else if title_upper.contains("DVD") || title_upper.contains("DVDRIP") {
                source = "DVD".to_string();
            } else if title_upper.contains("CAM") {
                source = "CAM".to_string();
            } else if title_upper.contains("TS") || title_upper.contains("TELESYNC") {
                source = "TS".to_string();
            } else if title_upper.contains("TC") || title_upper.contains("TELECINE") {
                source = "TC".to_string();
            } else if title_upper.contains("SCR") || title_upper.contains("SCREENER") {
                source = "SCR".to_string();
            }
            
            // Extract release group (handles [Group], -Group patterns)
            let release_group = extract_release_group(&t.title);
            
            // Extract container format
            let mut container = String::new();
            if title_upper.contains("MKV") {
                container = "MKV".to_string();
            } else if title_upper.contains("MP4") {
                container = "MP4".to_string();
            } else if title_upper.contains("AVI") {
                container = "AVI".to_string();
            } else if title_upper.contains("TS") && !source.eq("TS") {
                container = "TS".to_string();
            }
            
            // Build quality display with HDR info
            let quality_display = if !hdr_info.is_empty() {
                format!("{} {}", resolution, hdr_info.join("+"))
            } else {
                resolution.to_string()
            };
            
            // Format size nicely
            let size_str = if t.size_gb >= 10.0 {
                format!("{:.1} GB", t.size_gb)
            } else if t.size_gb >= 1.0 {
                format!("{:.2} GB", t.size_gb)
            } else {
                format!("{:.0} MB", t.size_gb * 1024.0)
            };
            
            // Build description parts
            let mut desc_parts = Vec::new();
            
            // Seeders/Leechers with arrows
            desc_parts.push(format!("⬆ {} ⬇ {}", t.seeders, t.leechers));
            
            // Codec
            if !codec.is_empty() {
                desc_parts.push(codec);
            }
            
            // Container
            if !container.is_empty() {
                desc_parts.push(container);
            }
            
            // Source
            if !source.is_empty() {
                desc_parts.push(source);
            }
            
            // Add RD indicator (placeholder - will be updated when resolved)
            desc_parts.push("RD ✓".to_string());
            
            // Build name with emoji icons
            let name = format!("{} 🎬 {} | {} | 🌱 {}", 
                quality_display, 
                size_str, 
                release_group,
                t.seeders
            );
            
            // Build description
            let description = desc_parts.join(" | ");
            
            Stream {
                name,
                description,
                info_hash: Some(t.info_hash.clone()),
                url: Some(format!("{}/resolve/{}", base_url, t.info_hash)),
                behavior_hints: serde_json::json!({
                    "bingeGroup": "torrent-".to_string() + &t.source,
                    "filename": t.title
                }),
            }
        })
        .collect();

    Json(StreamResponse { streams })
}

async fn resolve_handler(
    State(state): State<AppState>,
    Path(hash): Path<String>,
) -> Result<Redirect, (axum::http::StatusCode, String)> {
    info!("Resolve request for hash: {}", hash);

    // Check if we have cached resolved link
    if let Ok(Some(cached_url)) = db::get_cached_resolve(&state.db_pool, &hash).await {
        info!("Using cached resolved URL for {}", hash);
        return Ok(Redirect::temporary(&cached_url));
    }

    // Resolve via Real-Debrid
    match state.debrid_client.resolve_magnet(&hash).await {
        Ok(video_url) => {
            // Cache the resolved URL
            if let Err(e) = db::cache_resolve(&state.db_pool, &hash, &video_url).await {
                error!("Failed to cache resolved URL: {}", e);
            }
            
            Ok(Redirect::temporary(&video_url))
        }
        Err(e) => {
            error!("Failed to resolve magnet: {}", e);
            Err((
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to resolve: {}", e),
            ))
        }
    }
}