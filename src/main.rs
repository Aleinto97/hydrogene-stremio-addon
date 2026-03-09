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
use hydrogene::MetadataCache;

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
    dotenvy::dotenv().ok();
    
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"))
        .add_directive("tower_http=info".parse().unwrap())
        .add_directive("axum=info".parse().unwrap())
        .add_directive("scraper=info".parse().unwrap())
        .add_directive("selectors=info".parse().unwrap())
        .add_directive("html5ever=info".parse().unwrap())
        .add_directive("tendril=info".parse().unwrap())
        .add_directive("hyper=info".parse().unwrap())
        .add_directive("h2=info".parse().unwrap())
        .add_directive("rustls=info".parse().unwrap())
        .add_directive("sqlx=warn".parse().unwrap());
    
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .init();

    info!("Starting Stremio Addon Server...");

    let http_client = Arc::new(reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .pool_max_idle_per_host(10)
        .build()?);
    info!("HTTP client initialized with connection pooling");

    let scraper_manager = Arc::new(ScraperManager::new()?);
    
    let debrid_client = Arc::new(debrid::RealDebridClient::new()?);

    let metadata_client = if let Ok(database_url) = std::env::var("DATABASE_URL") {
        info!("Initializing metadata cache with PostgreSQL...");
        match MetadataCache::new(&database_url).await {
            Ok(cache) => {
                info!("Metadata cache initialized successfully");
                MetadataClient::new(http_client.clone())?.with_cache(Arc::new(cache))
            }
            Err(e) => {
                tracing::warn!("Failed to initialize metadata cache: {}. Continuing without cache.", e);
                MetadataClient::new(http_client.clone())?
            }
        }
    } else {
        info!("No DATABASE_URL found, running without metadata cache");
        MetadataClient::new(http_client.clone())?
    };

    let app_state = AppState {
        scraper_manager,
        debrid_client,
        metadata_client: Arc::new(metadata_client),
    };

    let app = Router::new()
        .route("/", get(root_handler))
        .route("/manifest.json", get(manifest_handler))
        .route("/stream/:type/:id.json", get(stream_handler))
        .route("/resolve/:hash/:id", get(resolve_handler))
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(|request: &axum::http::Request<_>| {
                    let uri = request.uri().to_string();
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

    let app = app.layer(axum::middleware::from_fn(timeout_middleware));
    
    let port = std::env::var("PORT")
        .unwrap_or_else(|_| "8080".to_string())
        .parse::<u16>()?;
    
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    info!("Server listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

async fn timeout_middleware(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let uri = req.uri().to_string();
    
    if uri == "/" || uri == "/manifest.json" || uri.starts_with("/resolve/") || uri.starts_with("/cached/") {
        return next.run(req).await;
    }
    
    match tokio::time::timeout(
        std::time::Duration::from_secs(8),
        next.run(req)
    ).await {
        Ok(response) => response,
        Err(_) => {
            tracing::warn!("Request timeout for {} after 8s", uri);
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

async fn stream_handler(
    State(state): State<AppState>,
    Path((content_type, id)): Path<(String, String)>,
) -> Json<StreamResponse> {
    let id = id.trim_end_matches(".json").to_string();
    
    let base_id = id.split(':').next().unwrap_or(&id).to_string();
    let metadata_id = if id.contains(':') && !id.starts_with("anilist:") {
        base_id.clone()
    } else {
        id.clone()
    };
    
    info!("Stream request: type={}, id={}", content_type, id);

    let (torrents, target_year, target_season, target_episode) = {
        let (target_season, target_episode) = if id.contains(':') && !id.starts_with("anilist:") {
            let parts: Vec<&str> = id.split(':').collect();
            if parts.len() >= 3 {
                (parts[1].parse::<u32>().ok(), parts[2].parse::<u32>().ok())
            } else {
                (None, None)
            }
        } else if id.starts_with("anilist:") {
            let parts: Vec<&str> = id.split(':').collect();
            if parts.len() >= 3 {
                (Some(1), parts[2].parse::<u32>().ok())
            } else {
                (None, None)
            }
        } else {
            (None, None)
        };

        let metadata_future = async {
            if metadata_id.starts_with("anilist:") {
                let parts: Vec<&str> = metadata_id.split(':').collect();
                let episode = if parts.len() >= 3 {
                    parts[2].parse::<u32>().ok()
                } else {
                    None
                };
                
                match state.metadata_client.lookup_by_imdb(&metadata_id, &content_type).await {
                    Ok(meta) => {
                        let mut queries = meta.search_queries;
                        if let Some(ep) = episode {
                            let ep_queries = build_anime_episode_queries(&queries, ep);
                            queries.extend(ep_queries);
                        }
                        (queries, meta.year)
                    }
                    Err(_) => (vec![metadata_id.clone()], None)
                }
            } else if metadata_id.starts_with("tt") {
                let full_metadata_id = if id.contains(':') && !id.starts_with("anilist:") {
                    id.clone()
                } else {
                    metadata_id.clone()
                };
                
                match state.metadata_client.lookup_by_imdb(&full_metadata_id, &content_type).await {
                    Ok(meta) => (meta.search_queries, meta.year),
                    Err(e) => {
                        tracing::error!("Metadata lookup failed for {}: {}. Falling back to ID search.", full_metadata_id, e);
                        (vec![metadata_id.clone()], None)
                    }
                }
            } else {
                (vec![metadata_id.clone()], None)
            }
        };

        let (search_queries, target_year) = metadata_future.await;
        
        let mut search_queries = search_queries;
        search_queries.sort_by(|a, b| {
            let a_exact = a.to_lowercase().contains(&id.to_lowercase());
            let b_exact = b.to_lowercase().contains(&id.to_lowercase());
            b_exact.cmp(&a_exact)
        });
        search_queries.dedup();
        search_queries.truncate(8);

        use futures::stream::{FuturesUnordered, StreamExt};
        let mut stream = FuturesUnordered::new();
        let mut all_torrents = Vec::new();
        
        for query in &search_queries {
            let manager = state.scraper_manager.clone();
            let q = query.clone();
            let ct = content_type.clone();
            
            stream.push(async move {
                info!("Scraping for query: {}", q);
                let scraped = manager.scrape_all(&q, &ct).await;
                (q, scraped)
            });
        }
        
        while let Some((query, scraped)) = stream.next().await {
            info!("Scraper found {} results for query: {}", scraped.len(), query);
            all_torrents.extend(scraped);
        }
        
        use std::collections::HashSet;
        let mut seen_hashes = HashSet::new();
        let mut unique: Vec<ScrapedTorrent> = all_torrents
            .into_iter()
            .filter(|t| seen_hashes.insert(t.info_hash.clone()))
            .collect();
        
        unique.sort_by(|a, b| b.seeders.cmp(&a.seeders));
        unique.truncate(50);
        
        info!("Found {} unique torrents for {}", unique.len(), id);
        (unique, target_year, target_season, target_episode)
    };

    let base_url = std::env::var("BASE_URL")
        .unwrap_or_else(|_| "http://torrentio-stack-aleinto97-54335f00.koyeb.app".to_string());

    let min_seeders: i32 = std::env::var("MIN_SEEDERS")
        .unwrap_or_else(|_| "1".to_string())
        .parse()
        .unwrap_or(1);
    
    let total_scraped = torrents.len();
    let filtered_torrents: Vec<ScrapedTorrent> = torrents
        .into_iter()
        .filter(|t| t.seeders >= min_seeders)
        .filter(|t| {
            if let (Some(target_season), Some(target_episode)) = (target_season, target_episode) {
                utils::is_exact_episode_match(&t.title, target_season, target_episode)
            } else {
                true
            }
        })
        .filter(|t| {
            // Filter by year if available (only for movies/series, not anime)
            if let Some(year) = &target_year {
                if !metadata_id.starts_with("anilist:") {
                    let title_upper = t.title.to_uppercase();
                    // Check if year is present in the title or filename
                    if !title_upper.contains(year) {
                        // If year is not in the title, still accept it but prefer results with year
                        // We'll handle preference in scoring
                        return true;
                    }
                }
            }
            true
        })
        .collect();
    
    let mut scored_torrents: Vec<(ScrapedTorrent, i32)> = filtered_torrents
        .into_iter()
        .map(|t| {
            let score = calculate_quality_score(&t.title, t.seeders, t.size_bytes, &target_year);
            (t, score)
        })
        .collect();
    
    scored_torrents.sort_by(|a, b| b.1.cmp(&a.1));
    
    let sorted_torrents: Vec<ScrapedTorrent> = scored_torrents
        .into_iter()
        .take(100)
        .map(|(t, _)| t)
        .collect();

    info!("Total unique torrents: {}, Top quality results: {}", total_scraped, sorted_torrents.len());

    let streams: Vec<StremioStream> = sorted_torrents
        .into_iter()
        .map(|t| {
            let info = TorrentInfo::from_scraped_torrent(&t);
            let mut stream = StremioStream::from_torrent_info(&info, &base_url);
            
            stream.url = Some(format!("{}/resolve/{}/{}", base_url, t.info_hash, id));
            
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

fn calculate_quality_score(title: &str, seeders: i32, size_bytes: u64, target_year: &Option<String>) -> i32 {
    let mut score = 0;
    let title_upper = title.to_uppercase();
    
    // Year matching bonus
    if let Some(year) = target_year {
        if title_upper.contains(year) {
            score += 20; // Strong bonus for matching year
        }
    }
    
    if title_upper.contains("2160P") || title_upper.contains("4K") || title_upper.contains("UHD") {
        score += 100;
    } else if title_upper.contains("1080P") {
        score += 80;
    } else if title_upper.contains("720P") {
        score += 60;
    } else if title_upper.contains("480P") || title_upper.contains("360P") {
        score += 20;
    }
    
    if title_upper.contains("BLURAY") || title_upper.contains("BDRIP") {
        score += 30;
    } else if title_upper.contains("WEB-DL") || title_upper.contains("WEBDL") {
        score += 25;
    } else if title_upper.contains("HDTV") {
        score += 15;
    }
    
    if title_upper.contains("HEVC") || title_upper.contains("X265") {
        score += 10;
    }
    
    score += (seeders / 10).min(20) as i32;
    
    let size_gb = size_bytes as f64 / 1_073_741_824.0;
    if size_gb > 5.0 && size_gb < 20.0 {
        score += 10;
    }
    
    score
}

fn build_anime_episode_queries(base_titles: &[String], episode: u32) -> Vec<String> {
    let mut queries = Vec::new();
    
    for title in base_titles {
        queries.push(format!("{} {:02}", title, episode));
        queries.push(format!("{} - {:02}", title, episode));
        queries.push(format!("{} E{:02}", title, episode));
        queries.push(format!("{} EP{:02}", title, episode));
        
        if episode < 10 {
            queries.push(format!("{} {}", title, episode));
            queries.push(format!("{} - {}", title, episode));
        }
    }
    
    queries
}

async fn resolve_handler(
    State(state): State<AppState>,
    Path((hash, id)): Path<(String, String)>,
) -> Result<Redirect, (axum::http::StatusCode, String)> {
    info!("Resolve request for hash: {}, id: {}", hash, id);
    
    let (season, episode) = if id.contains(':') && !id.starts_with("anilist:") {
        let parts: Vec<&str> = id.split(':').collect();
        if parts.len() >= 3 {
            (parts[1].parse::<u32>().ok(), parts[2].parse::<u32>().ok())
        } else {
            (None, None)
        }
    } else if id.starts_with("anilist:") {
        let parts: Vec<&str> = id.split(':').collect();
        if parts.len() >= 3 {
            (Some(1), parts[2].parse::<u32>().ok())
        } else {
            (Some(1), None)
        }
    } else {
        (None, None)
    };
    
    if season.is_some() || episode.is_some() {
        info!("Target for pack selection: S{:?}E{:?}", season, episode);
    }

    match tokio::time::timeout(
        std::time::Duration::from_secs(300),
        state.debrid_client.resolve_magnet_with_status(&hash, season, episode)
    ).await {
        Ok(Ok(ResolveResult::Ready(video_url))) => {
            info!("Torrent {} ready, redirecting to video", hash);
            Ok(Redirect::temporary(&video_url))
        }
        Ok(Ok(ResolveResult::Downloading(progress))) => {
            info!("Torrent {} is downloading ({}%)", hash, progress);
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
                "⏳ Torrent in elaborazione su Real-Debrid. Riprova tra 1-2 minuti.".to_string(),
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
