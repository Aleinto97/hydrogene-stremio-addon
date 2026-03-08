use sqlx::{PgPool, postgres::PgPoolOptions, Row};
use std::time::{SystemTime, UNIX_EPOCH};
use anyhow::Result;
use crate::scrapers::ScrapedTorrent;

pub type DbPool = PgPool;

pub async fn init_pool() -> Result<DbPool> {
    let database_url = std::env::var("DATABASE_URL")
        .expect("DATABASE_URL must be set");

    let pool = PgPoolOptions::new()
        .max_connections(10)
        .min_connections(2)
        .acquire_timeout(std::time::Duration::from_secs(5))
        .connect(&database_url)
        .await?;

    // Run migrations
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await?;

    Ok(pool)
}

pub async fn get_cached_torrents(
    pool: &DbPool,
    imdb_id: &str,
) -> Result<Vec<ScrapedTorrent>> {
    let ttl_hours: i64 = std::env::var("CACHE_TTL_HOURS")
        .unwrap_or_else(|_| "24".to_string())
        .parse()?;
    
    let cutoff = SystemTime::now()
        .duration_since(UNIX_EPOCH)?
        .as_secs() as i64 - (ttl_hours * 3600);

    let rows = sqlx::query(
        r#"
        SELECT title, info_hash, magnet_link, size_bytes, seeders, leechers, source, category
        FROM torrent_cache
        WHERE imdb_id = $1 AND created_at > to_timestamp($2)
        ORDER BY seeders DESC
        "#
    )
    .bind(imdb_id)
    .bind(cutoff)
    .fetch_all(pool)
    .await?;

    let mut torrents = Vec::new();
    for row in rows {
        let size_bytes: i64 = row.try_get("size_bytes").unwrap_or(0);
        
        torrents.push(ScrapedTorrent {
            title: row.try_get("title").unwrap_or_default(),
            info_hash: row.try_get("info_hash").unwrap_or_default(),
            magnet_link: row.try_get("magnet_link").unwrap_or_default(),
            size_bytes: size_bytes as u64,
            size_gb: size_bytes as f64 / 1_073_741_824.0,
            seeders: row.try_get("seeders").unwrap_or(0),
            leechers: row.try_get("leechers").unwrap_or(0),
            source: row.try_get("source").unwrap_or_default(),
            category: row.try_get("category").unwrap_or_else(|_| "Unknown".to_string()),
        });
    }

    Ok(torrents)
}

pub async fn cache_torrents(
    pool: &DbPool,
    imdb_id: &str,
    torrents: &[ScrapedTorrent],
) -> Result<()> {
    let mut tx = pool.begin().await?;

    // Delete old entries
    sqlx::query("DELETE FROM torrent_cache WHERE imdb_id = $1")
        .bind(imdb_id)
        .execute(&mut *tx)
        .await?;

    // Insert new entries
    for torrent in torrents {
        sqlx::query(
            r#"
            INSERT INTO torrent_cache 
            (imdb_id, title, info_hash, magnet_link, size_bytes, seeders, leechers, source, category)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            ON CONFLICT (imdb_id, info_hash, source) 
            DO UPDATE SET 
                title = $2,
                magnet_link = $3,
                size_bytes = $5,
                seeders = $6,
                leechers = $7,
                category = $9,
                updated_at = CURRENT_TIMESTAMP
            "#
        )
        .bind(imdb_id)
        .bind(&torrent.title)
        .bind(&torrent.info_hash)
        .bind(&torrent.magnet_link)
        .bind(torrent.size_bytes as i64)
        .bind(torrent.seeders)
        .bind(torrent.leechers)
        .bind(&torrent.source)
        .bind(&torrent.category)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    Ok(())
}

pub async fn get_cached_resolve(
    pool: &DbPool,
    info_hash: &str,
) -> Result<Option<String>> {
    let ttl_hours: i64 = std::env::var("CACHE_TTL_HOURS")
        .unwrap_or_else(|_| "24".to_string())
        .parse()?;
    
    let cutoff = SystemTime::now()
        .duration_since(UNIX_EPOCH)?
        .as_secs() as i64 - (ttl_hours * 3600);

    let row = sqlx::query(
        r#"
        SELECT video_url
        FROM resolve_cache
        WHERE info_hash = $1 AND created_at > to_timestamp($2)
        "#
    )
    .bind(info_hash)
    .bind(cutoff)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| r.try_get("video_url").unwrap_or_default()))
}

pub async fn cache_resolve(
    pool: &DbPool,
    info_hash: &str,
    video_url: &str,
) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO resolve_cache (info_hash, video_url)
        VALUES ($1, $2)
        ON CONFLICT (info_hash) 
        DO UPDATE SET video_url = $2, created_at = CURRENT_TIMESTAMP
        "#
    )
    .bind(info_hash)
    .bind(video_url)
    .execute(pool)
    .await?;

    Ok(())
}