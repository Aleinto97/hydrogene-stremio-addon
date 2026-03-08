use axum::{
    routing::{get, post},
    Router,
    extract::{Path, State},
    response::{Json, Redirect},
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

    // Initialize scraper manager
    let scraper_manager = Arc::new(ScraperManager::new()?);
    
    // Initialize Real-Debrid client
    let debrid_client = Arc::new(debrid::RealDebridClient::new()?);

    // Initialize metadata client
    let metadata_client = Arc::new(MetadataClient::new()?);

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
    info_hash: Option<String>,
    url: Option<String>,
    #[serde(rename = "behaviorHints")]
    behavior_hints: serde_json::Value,
}

async fn stream_handler(
    State(state): State<AppState>,
    Path((content_type, id)): Path<(String, String)>,
) -> Json<StreamResponse> {
    eprintln!("DEBUG: Stream request: type={}, id={}", content_type, id);
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
            eprintln!("DEBUG: Cache hit for {}", id);
            info!("Cache hit for {}", id);
            cached_torrents
        } else {
            eprintln!("DEBUG: Cache empty for {}", id);
            vec![]
        }
    } else {
        eprintln!("DEBUG: Cache miss for {}, looking up metadata...", id);
        info!("Cache miss for {}, looking up metadata...", id);
        
        // Check if TMDB key is configured
        let tmdb_key = std::env::var("TMDB_API_KEY").ok();
        eprintln!("DEBUG: TMDB_API_KEY present: {}", tmdb_key.is_some());
        
        // Lookup metadata to get title
        let metadata = match state.metadata_client.lookup_by_imdb(&id, &content_type).await {
            Ok(meta) => {
                eprintln!("DEBUG: Found metadata: {} ({})", meta.title, meta.year.as_ref().unwrap_or(&"N/A".to_string()));
                info!("Found metadata: {} ({})", meta.title, meta.year.as_ref().unwrap_or(&"N/A".to_string()));
                meta
            }
            Err(e) => {
                eprintln!("DEBUG: Failed to lookup metadata for {}: {}", id, e);
                error!("Failed to lookup metadata for {}: {}", id, e);
                // Fallback: use ID as search query
                metadata::ContentMetadata {
                    title: id.clone(),
                    year: None,
                    content_type: content_type.clone(),
                    search_queries: vec![id.clone()],
                }
            }
        };

        // Try scraping with different search queries
        let mut all_torrents = Vec::new();
        
        eprintln!("DEBUG: Will try {} queries", metadata.search_queries.len());
        for query in &metadata.search_queries {
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
    };

    // Convert to Stremio streams
    let streams: Vec<Stream> = torrents
        .into_iter()
        .map(|t| Stream {
            name: format!("🎬 {} ({} peers)", t.title, t.seeders + t.leechers),
            description: format!(
                "📦 {:.2} GB | ⬆ {} ⬇ {} | 🏷 {} | 🔍 {}",
                t.size_gb,
                t.seeders,
                t.leechers,
                t.source,
                id
            ),
            info_hash: Some(t.info_hash.clone()),
            url: Some(format!("/resolve/{}", t.info_hash)),
            behavior_hints: serde_json::json!({
                "bingeGroup": "torrent-".to_string() + &t.source,
                "filename": t.title
            }),
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