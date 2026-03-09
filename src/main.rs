use axum::{
    routing::get,
    Router,
    extract::{Path, State},
    response::{Json, Redirect},
};
use std::net::SocketAddr;
use std::sync::Arc;
use tracing::{info, error};
use tower_http::trace::TraceLayer;

use hydrogene::debrid;
use hydrogene::metadata;
use hydrogene::scrapers;
use hydrogene::stremio_format::{StremioStream, TorrentInfo};
use hydrogene::utils;
use hydrogene::ResolveResult;

use metadata::MetadataClient;
use scrapers::{ScraperManager, ScrapedTorrent};

#[derive(Clone)]
struct AppState {
    scraper_manager: Arc<ScraperManager>,
    debrid_client: Arc<debrid::RealDebridClient>,
    metadata_client: Arc<MetadataClient>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load environment variables
    dotenvy::dotenv().ok();
    
    // Initialize tracing with a custom filter
    // Default to info, but suppress debug logs from tower_http and other verbose dependencies
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,tower_http=info,axum=info"));
    
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .init();

    info!("Starting Stremio Addon Server...");

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
        scraper_manager,
        debrid_client,
        metadata_client,
    };

    // Build router
    let app = Router::new()
        .route("/", get(root_handler))
        .route("/manifest.json", get(manifest_handler))
        .route("/stream/:type/:id.json", get(stream_handler))
        .route("/cached/:type/:id.json", get(cached_handler))
        .route("/resolve/:hash", get(resolve_handler))
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(|request: &axum::http::Request<_>| {
                    let uri = request.uri().to_string();
                    // Do not create spans for health check on root path to keep logs clean
                    if uri == "/" {
                        tracing::Span::none()
                    } else {
                        tracing::info_span!(
                            "request",
                            method = %request.method(),
                            uri = %uri,
                        )
                    }
                })
        )
        .layer(tower_http::cors::CorsLayer::permissive())
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
    
    // Skip timeout for health checks, manifest, resolve, and cached endpoints
    if uri == "/" || uri == "/manifest.json" || uri.starts_with("/resolve/") || uri.starts_with("/cached/") {
        return next.run(req).await;
    }
    
    // Apply 10 second timeout for stream requests (fast response is critical)
    match tokio::time::timeout(
        std::time::Duration::from_secs(10),
        next.run(req)
    ).await {
        Ok(response) => response,
        Err(_) => {
            tracing::warn!("Request timeout for {} after 10s", uri);
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
        id_prefixes: vec!["tt".to_string(), "anilist".to_string()],
        behavior_hints: serde_json::json!({
            "configurable": false,
            "configurationRequired": false
        }),
    })
}

#[derive(serde::Serialize)]
struct StreamResponse {
    streams: Vec<StremioStream>,
}

// Extract release group from torrent title
// Handles patterns like [Group], -Group, .Group, or Group at end
fn extract_release_group(title: &str) -> String {
    
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
    // - Anime: anilist:16498
    // Extract base ID for metadata lookup
    let base_id = id.split(':').next().unwrap_or(&id).to_string();
    let metadata_id = if id.contains(':') && !id.starts_with("anilist:") {
        // For series episodes (tt:season:episode format), use base ID (tt0903747)
        base_id.clone()
    } else {
        // For movies and anime (anilist:), use full ID
        id.clone()
    };
    
    info!("Stream request: type={}, id={}", content_type, id);

    // Direct search without cache checking - results returned immediately
    let torrents = {
        // Determine search queries
        let search_queries: Vec<String> =         if metadata_id.starts_with("anilist:") {
            // --- ANIME: Use GraphQL to resolve title ---
            
            // Extract episode number from ID if present (format: anilist:ID:EP)
            let parts: Vec<&str> = metadata_id.split(':').collect();
            
            let episode = if parts.len() >= 3 {
                parts[2].parse::<u32>().ok()
            } else {
                None
            };
            
            // Resolve anime title using GraphQL
            match state.metadata_client.resolve_anime_title(&metadata_id).await {
                Some(title) => {
                    // Build queries with episode info
                    let mut queries = vec![title.clone()];
                    
                    if let Some(ep) = episode {
                        // Add episode-specific queries
                        queries.push(format!("{} {:02}", title, ep));
                        queries.push(format!("{} E{:02}", title, ep));
                    }
                    
                    queries
                }
                None => {
                    vec![metadata_id.clone()]
                }
            }
        } else if metadata_id.starts_with("tt") {
            // --- MOVIES/SERIES: Use TMDB ---
            // Pass full ID including season:episode for series (format: tt1234567:1:3)
            let full_metadata_id = if id.contains(':') && !id.starts_with("anilist:") {
                id.clone()  // Use full ID with season:episode
            } else {
                metadata_id.clone()
            };
            
            match state.metadata_client.lookup_by_imdb(&full_metadata_id, &content_type).await {
                Ok(meta) => {
                    meta.search_queries
                }
                Err(e) => {
                    tracing::error!("Metadata lookup failed for {}: {}. Falling back to ID search.", full_metadata_id, e);
                    vec![metadata_id.clone()]
                }
            }
        } else {
            // --- DIRECT TITLE SEARCH ---
            vec![metadata_id.clone()]
        };

        // Try scraping with different search queries
        let mut all_torrents = Vec::new();
        
        for query in &search_queries {
            info!("Scraping for query: {}", query);
            let scraped = state.scraper_manager.scrape_all(query, &content_type).await;
            info!("Scraper found {} results for query: {}", scraped.len(), query);
            all_torrents.extend(scraped);
            
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
        
        info!("Found {} unique torrents for {}", unique.len(), id);
        unique
    };

    // Convert to Stremio streams immediately without cache checking
    // Get base URL for absolute URLs
    let base_url = std::env::var("BASE_URL")
        .unwrap_or_else(|_| "http://torrentio-stack-aleinto97-54335f00.koyeb.app".to_string());
    
    // Parse the ID to extract season and episode for series
    let (target_season, target_episode) = if id.contains(':') && !id.starts_with("anilist:") {
        let parts: Vec<&str> = id.split(':').collect();
        if parts.len() >= 3 {
            let season = parts[1].parse::<u32>().ok();
            let episode = parts[2].parse::<u32>().ok();
            (season, episode)
        } else {
            (None, None)
        }
    } else if id.starts_with("anilist:") {
        // For anime, extract from anilist ID - but we don't have season/ep info
        (None, None)
    } else {
        (None, None)
    };

    // Filter torrents by quality (only 1080p, 2160p, 4K, UHD), minimum seeders, and season/episode matching
    let min_seeders: i32 = std::env::var("MIN_SEEDERS")
        .unwrap_or_else(|_| "1".to_string())
        .parse()
        .unwrap_or(1);
    
    let total_scraped = torrents.len();
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
                // Use regex with word boundaries to prevent false matches (e.g., S01E01 matching S01E016)
                utils::is_exact_episode_match(&t.title, target_season, target_episode)
            } else {
                true // No specific season/episode, include all
            }
        })
        .collect();
    
    info!("Total unique torrents: {}, Filtered (quality/episode): {}", total_scraped, filtered_torrents.len());
    if filtered_torrents.is_empty() && total_scraped > 0 {
        info!("No torrents matched the filters (min seeders: {}, quality: 1080p+, episode match: {:?}:{:?})", 
            min_seeders, target_season, target_episode);
    }

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

    // Convert to Stremio streams using new formatter
    let streams: Vec<StremioStream> = sorted_torrents
        .into_iter()
        .map(|t| {
            let info = TorrentInfo::from_scraped_torrent(&t);
            let mut stream = StremioStream::from_torrent_info(&info, &base_url);
            // Aggiungi [RD] all'inizio del name per indicare Real-Debrid
            stream.name = format!("[RD]+\n{}", stream.name);
            stream.behavior_hints = serde_json::json!({
                "bingeGroup": format!("torrent-{}", t.source),
                "filename": t.title
            });
            stream
        })
        .collect();

    Json(StreamResponse { streams })
}

// New handler that only shows torrents already cached on Real-Debrid
async fn cached_handler(
    State(state): State<AppState>,
    Path((content_type, id)): Path<(String, String)>,
) -> Json<StreamResponse> {
    let id = id.trim_end_matches(".json").to_string();
    
    info!("Cached search request: type={}, id={}", content_type, id);
    
    // Get metadata for better search
    let search_queries = if id.starts_with("tt") {
        match state.metadata_client.lookup_by_imdb(&id, &content_type).await {
            Ok(meta) => {
                info!("Found metadata: {} ({} queries)", meta.title, meta.search_queries.len());
                meta.search_queries
            }
            Err(_) => vec![id.clone()],
        }
    } else {
        vec![id.clone()]
    };
    
    // Search for torrents
    let mut all_torrents = Vec::new();
    
    for query in &search_queries {
        info!("Scraping for query: {}", query);
        let scraped = state.scraper_manager.scrape_all(query, &content_type).await;
        info!("Scraped {} torrents for query {}", scraped.len(), query);
        all_torrents.extend(scraped);
    }
    
    // Remove duplicates and sort by seeders
    use std::collections::HashSet;
    let mut seen_hashes = HashSet::new();
    let mut unique: Vec<ScrapedTorrent> = all_torrents
        .into_iter()
        .filter(|t| seen_hashes.insert(t.info_hash.clone()))
        .collect();
    
    unique.sort_by(|a, b| b.seeders.cmp(&a.seeders));
    
    // Filter for high quality only
    let quality_torrents: Vec<ScrapedTorrent> = unique
        .into_iter()
        .filter(|t| {
            let title_upper = t.title.to_uppercase();
            title_upper.contains("1080P") || 
            title_upper.contains("2160P") || 
            title_upper.contains("4K") ||
            title_upper.contains("UHD") ||
            title_upper.contains("BLURAY")
        })
        .take(15) // Check top 15 for speed
        .collect();
    
    info!("Checking {} quality torrents for cache status", quality_torrents.len());
    
    // Check which ones are cached on Real-Debrid
    let cached_torrents = match state.debrid_client.check_batch_cache(&quality_torrents).await {
        Ok(torrents) => {
            let cached: Vec<_> = torrents.into_iter().filter(|t| t.is_cached).collect();
            info!("Found {} cached torrents", cached.len());
            cached
        }
        Err(e) => {
            tracing::error!("Failed to check cache: {}", e);
            vec![]
        }
    };
    
    // Convert to Stremio streams
    let base_url = std::env::var("BASE_URL")
        .unwrap_or_else(|_| "http://localhost:8080".to_string());
    
    let streams: Vec<StremioStream> = cached_torrents
        .into_iter()
        .map(|t| {
            let info = TorrentInfo::from_scraped_torrent(&t);
            let mut stream = StremioStream::from_torrent_info(&info, &base_url);
            stream.name = format!("[RD]+ ✅ CACHED\n{}", stream.name);
            stream.behavior_hints = serde_json::json!({
                "bingeGroup": format!("torrent-{}", t.source),
                "filename": t.title,
                "cached": true,
                "ready": true
            });
            stream
        })
        .collect();

    Json(StreamResponse { streams })
}

async fn resolve_handler(
    State(state): State<AppState>,
    Path(hash): Path<String>,
) -> Result<Redirect, (axum::http::StatusCode, String)> {
    info!("Resolve request for hash: {}", hash);

    // Add magnet to RD and check immediate status (no pre-check)
    // Longer timeout for user clicks - up to 5 minutes for download
    match tokio::time::timeout(
        std::time::Duration::from_secs(300), // 5 minutes
        state.debrid_client.resolve_magnet_with_status(&hash)
    ).await {
        Ok(Ok(ResolveResult::Ready(video_url))) => {
            info!("Torrent {} ready, redirecting to video", hash);
            Ok(Redirect::temporary(&video_url))
        }
        Ok(Ok(ResolveResult::Downloading(progress))) => {
            info!("Torrent {} is downloading ({}%)", hash, progress);
            // Return error with user-friendly message about download status
            Err((
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                format!("⏳ Torrent in download su Real-Debrid ({}% completato). Riprova tra 2-3 minuti.", progress),
            ))
        }
        Ok(Ok(ResolveResult::Queued)) => {
            info!("Torrent {} is queued for download", hash);
            Err((
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "⏳ Torrent in coda su Real-Debrid. Riprova tra 2-3 minuti.".to_string(),
            ))
        }
        Ok(Ok(ResolveResult::Processing)) => {
            info!("Torrent {} is processing metadata", hash);
            Err((
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "⏳ Torrent in elaborazione su Real-Debrid. Riproza tra 1-2 minuti.".to_string(),
            ))
        }
        Ok(Err(e)) => {
            error!("Failed to resolve {}: {}", hash, e);
            Err((
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                format!("❌ Errore: {}. Il torrent potrebbe non essere disponibile.", e),
            ))
        }
        Err(_) => {
            error!("Timeout for {} after 5 minutes", hash);
            Err((
                axum::http::StatusCode::REQUEST_TIMEOUT,
                "⏱️ Timeout: il torrent sta ancora scaricando su Real-Debrid. Riprova tra qualche minuto.".to_string(),
            ))
        }
    }
}