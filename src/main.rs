use axum::{
    extract::{Path, State},
    response::{Json, Redirect},
    routing::get,
    Router,
};
use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::Arc;
use tower_http::trace::TraceLayer;
use tracing::{error, info};

use hydrogene::debrid;
use hydrogene::metadata;
use hydrogene::scrapers;
use hydrogene::stremio_format::{StremioStream, TorrentInfo};
use hydrogene::MetadataCache;
use hydrogene::ResolveResult;

use metadata::MetadataClient;
use scrapers::{ScrapedTorrent, ScraperManager};

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

    tracing_subscriber::fmt().with_env_filter(filter).init();

    info!("Starting Stremio Addon Server...");

    let http_client = Arc::new(
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .pool_max_idle_per_host(10)
            .build()?,
    );
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
                tracing::warn!(
                    "Failed to initialize metadata cache: {}. Continuing without cache.",
                    e
                );
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
            TraceLayer::new_for_http().make_span_with(|request: &axum::http::Request<_>| {
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
            }),
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

    if uri == "/"
        || uri == "/manifest.json"
        || uri.starts_with("/resolve/")
        || uri.starts_with("/cached/")
    {
        return next.run(req).await;
    }

    match tokio::time::timeout(std::time::Duration::from_secs(8), next.run(req)).await {
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
    let addon_name =
        std::env::var("ADDON_NAME").unwrap_or_else(|_| "Hydrogen Torrents".to_string());
    let addon_desc = std::env::var("ADDON_DESCRIPTION")
        .unwrap_or_else(|_| "High-performance torrent scraper".to_string());
    let addon_id = std::env::var("ADDON_ID").unwrap_or_else(|_| "ai.hydrogen.stremio".to_string());
    let addon_version = std::env::var("ADDON_VERSION").unwrap_or_else(|_| "0.1.0".to_string());

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

    let (torrents, metadata_title, target_year, target_season, target_episode) = {
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
                match state
                    .metadata_client
                    .lookup_by_imdb(&metadata_id, &content_type)
                    .await
                {
                    Ok(meta) => (meta.title, meta.search_queries, meta.year),
                    Err(_) => (metadata_id.clone(), vec![metadata_id.clone()], None),
                }
            } else if metadata_id.starts_with("tt") {
                let full_metadata_id = if id.contains(':') && !id.starts_with("anilist:") {
                    id.clone()
                } else {
                    metadata_id.clone()
                };

                match state
                    .metadata_client
                    .lookup_by_imdb(&full_metadata_id, &content_type)
                    .await
                {
                    Ok(meta) => (meta.title, meta.search_queries, meta.year),
                    Err(e) => {
                        tracing::error!(
                            "Metadata lookup failed for {}: {}. Falling back to ID search.",
                            full_metadata_id,
                            e
                        );
                        (metadata_id.clone(), vec![metadata_id.clone()], None)
                    }
                }
            } else {
                (metadata_id.clone(), vec![metadata_id.clone()], None)
            }
        };

        let (metadata_title, search_queries, target_year) = metadata_future.await;
        let target_year_u32 = target_year.as_ref().and_then(|y| y.parse::<u32>().ok());

        let mut search_queries = search_queries;
        search_queries.sort_by(|a, b| {
            let a_score = score_search_query(a, &metadata_title, target_year_u32, target_episode);
            let b_score = score_search_query(b, &metadata_title, target_year_u32, target_episode);
            b_score.cmp(&a_score)
        });
        search_queries.dedup();
        search_queries.truncate(8);

        let mut unique = scrape_queries_progressively(
            state.scraper_manager.clone(),
            &content_type,
            &search_queries,
            &metadata_title,
            target_year_u32,
            target_season,
            target_episode,
        )
        .await;

        unique.sort_by(|a, b| {
            let b_score = hydrogene::calculate_match_score(
                &metadata_title,
                target_year_u32,
                target_season,
                target_episode,
                &b.title,
                b.seeders,
                b.size_bytes,
            );
            let a_score = hydrogene::calculate_match_score(
                &metadata_title,
                target_year_u32,
                target_season,
                target_episode,
                &a.title,
                a.seeders,
                a.size_bytes,
            );
            b_score.cmp(&a_score)
        });
        unique.truncate(80);

        info!("Found {} unique torrents for {}", unique.len(), id);
        (
            unique,
            metadata_title,
            target_year,
            target_season,
            target_episode,
        )
    };

    let base_url = std::env::var("BASE_URL")
        .unwrap_or_else(|_| "http://torrentio-stack-aleinto97-54335f00.koyeb.app".to_string());

    let min_seeders: i32 = std::env::var("MIN_SEEDERS")
        .unwrap_or_else(|_| "1".to_string())
        .parse()
        .unwrap_or(1);

    let total_scraped = torrents.len();

    // Parse target year from string to u32 for matching
    let target_year_u32 = target_year.as_ref().and_then(|y| y.parse::<u32>().ok());

    // Score all torrents (don't filter out season packs)
    let mut scored_torrents: Vec<(ScrapedTorrent, i32)> = torrents
        .into_iter()
        .filter(|t| t.seeders >= min_seeders)
        .map(|t| {
            // Use advanced matching with fuzzy logic
            let score = hydrogene::calculate_match_score(
                &metadata_title, // Query title
                target_year_u32, // Target year
                target_season,   // Target season
                target_episode,  // Target episode
                &t.title,        // Torrent title
                t.seeders,       // Seeders
                t.size_bytes,    // Size
            );
            (t, score)
        })
        .collect();

    scored_torrents.sort_by(|a, b| b.1.cmp(&a.1));

    let sorted_torrents: Vec<ScrapedTorrent> = scored_torrents
        .into_iter()
        .take(100)
        .map(|(t, _)| t)
        .collect();

    info!(
        "Total unique torrents: {}, Top quality results: {}",
        total_scraped,
        sorted_torrents.len()
    );

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
        let episode_markers = [
            format!("e{:02}", episode),
            format!("ep{:02}", episode),
            format!(" {:02}", episode),
            format!(" {}", episode),
        ];

        if episode_markers
            .iter()
            .any(|marker| query_lower.contains(marker))
        {
            score += 30;
        }
    }

    score - query.len() as i32 / 8
}

async fn scrape_queries_progressively(
    scraper_manager: Arc<ScraperManager>,
    content_type: &str,
    search_queries: &[String],
    metadata_title: &str,
    target_year: Option<u32>,
    target_season: Option<u32>,
    target_episode: Option<u32>,
) -> Vec<ScrapedTorrent> {
    let mut all_torrents = Vec::new();
    let query_batches = build_query_batches(search_queries, target_episode);

    for (batch_index, batch) in query_batches.iter().enumerate() {
        let scraped = scrape_query_batch(scraper_manager.clone(), content_type, batch).await;
        all_torrents.extend(scraped);

        let unique = dedupe_torrents_by_hash(all_torrents.clone());

        if batch_index + 1 < query_batches.len()
            && should_stop_query_expansion(
                &unique,
                metadata_title,
                target_year,
                target_season,
                target_episode,
            )
        {
            info!(
                "Stopping query expansion early after batch {}/{} with {} unique torrents",
                batch_index + 1,
                query_batches.len(),
                unique.len()
            );
            return unique;
        }
    }

    dedupe_torrents_by_hash(all_torrents)
}

async fn scrape_query_batch(
    scraper_manager: Arc<ScraperManager>,
    content_type: &str,
    queries: &[String],
) -> Vec<ScrapedTorrent> {
    use futures::stream::{FuturesUnordered, StreamExt};

    let mut stream = FuturesUnordered::new();

    for query in queries {
        let manager = scraper_manager.clone();
        let query = query.clone();
        let content_type = content_type.to_string();

        stream.push(async move {
            info!("Scraping for query: {}", query);
            let scraped = manager.scrape_all(&query, &content_type).await;
            (query, scraped)
        });
    }

    let mut all_torrents = Vec::new();
    while let Some((query, scraped)) = stream.next().await {
        info!(
            "Scraper found {} results for query: {}",
            scraped.len(),
            query
        );
        all_torrents.extend(scraped);
    }

    all_torrents
}

fn build_query_batches(search_queries: &[String], target_episode: Option<u32>) -> Vec<Vec<String>> {
    match search_queries.len() {
        0 => Vec::new(),
        1 => vec![search_queries.to_vec()],
        _ if target_episode.is_some() => {
            let mut batches = vec![vec![search_queries[0].clone()]];

            if search_queries.len() > 1 {
                batches.push(search_queries[1..search_queries.len().min(3)].to_vec());
            }

            if search_queries.len() > 3 {
                batches.push(search_queries[3..].to_vec());
            }

            batches
        }
        2 => vec![search_queries.to_vec()],
        3 | 4 => vec![search_queries[..2].to_vec(), search_queries[2..].to_vec()],
        _ => vec![
            search_queries[..2].to_vec(),
            search_queries[2..4].to_vec(),
            search_queries[4..].to_vec(),
        ],
    }
}

fn dedupe_torrents_by_hash(torrents: Vec<ScrapedTorrent>) -> Vec<ScrapedTorrent> {
    let mut seen_hashes = HashSet::new();
    torrents
        .into_iter()
        .filter(|torrent| seen_hashes.insert(torrent.info_hash.clone()))
        .collect()
}

fn should_stop_query_expansion(
    torrents: &[ScrapedTorrent],
    metadata_title: &str,
    target_year: Option<u32>,
    target_season: Option<u32>,
    target_episode: Option<u32>,
) -> bool {
    if torrents.len() < 12 {
        return false;
    }

    let mut scores: Vec<i32> = torrents
        .iter()
        .map(|torrent| {
            hydrogene::calculate_match_score(
                metadata_title,
                target_year,
                target_season,
                target_episode,
                &torrent.title,
                torrent.seeders,
                torrent.size_bytes,
            )
        })
        .collect();

    scores.sort_unstable_by(|a, b| b.cmp(a));

    let strong_threshold = if target_episode.is_some() {
        105
    } else if target_season.is_some() {
        85
    } else {
        70
    };

    let strong_count = scores
        .iter()
        .take(12)
        .filter(|score| **score >= strong_threshold)
        .count();
    let top_score = scores.first().copied().unwrap_or_default();

    strong_count >= 6 || (strong_count >= 4 && top_score >= strong_threshold + 15)
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
        state
            .debrid_client
            .resolve_magnet_with_status(&hash, season, episode),
    )
    .await
    {
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
                format!(
                    "❌ Errore: {}. Il torrent potrebbe non essere disponibile.",
                    e
                ),
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
