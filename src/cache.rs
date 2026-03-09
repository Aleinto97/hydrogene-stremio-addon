use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

const CACHE_TTL_HOURS: i64 = 24;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedMetadata {
    pub title: String,
    pub year: Option<String>,
    pub content_type: String,
    pub search_queries: Vec<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone)]
pub struct MetadataCache {
    pool: PgPool,
}

impl MetadataCache {
    pub async fn new(database_url: &str) -> Result<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .min_connections(1)
            .connect(database_url)
            .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS metadata_cache (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                year TEXT,
                content_type TEXT NOT NULL,
                search_queries JSONB NOT NULL,
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
            )
        "#,
        )
        .execute(&pool)
        .await?;

        Ok(Self { pool })
    }

    pub async fn get(&self, id: &str) -> Result<Option<CachedMetadata>> {
        let result = sqlx::query_as::<_, (String, Option<String>, String, serde_json::Value, DateTime<Utc>)>(
            "SELECT title, year, content_type, search_queries, created_at FROM metadata_cache WHERE id = $1"
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        match result {
            Some((title, year, content_type, queries_json, created_at)) => {
                let search_queries: Vec<String> = serde_json::from_value(queries_json)?;
                let cached = CachedMetadata {
                    title,
                    year,
                    content_type,
                    search_queries,
                    created_at,
                };

                if is_cache_entry_fresh(&cached) {
                    tracing::info!("Cache HIT for metadata: {}", id);
                    Ok(Some(cached))
                } else {
                    tracing::info!("Cache EXPIRED for metadata: {}", id);
                    Ok(None)
                }
            }
            None => {
                tracing::info!("Cache MISS for metadata: {}", id);
                Ok(None)
            }
        }
    }

    pub async fn set(&self, id: &str, metadata: &CachedMetadata) -> Result<()> {
        let queries_json = serde_json::to_value(&metadata.search_queries)?;

        sqlx::query(
            "INSERT INTO metadata_cache (id, title, year, content_type, search_queries, created_at)
             VALUES ($1, $2, $3, $4, $5, $6)
             ON CONFLICT (id) DO UPDATE SET
                title = EXCLUDED.title,
                year = EXCLUDED.year,
                content_type = EXCLUDED.content_type,
                search_queries = EXCLUDED.search_queries,
                created_at = EXCLUDED.created_at",
        )
        .bind(id)
        .bind(&metadata.title)
        .bind(&metadata.year)
        .bind(&metadata.content_type)
        .bind(&queries_json)
        .bind(metadata.created_at)
        .execute(&self.pool)
        .await?;

        tracing::info!("Cache SET for metadata: {}", id);
        Ok(())
    }
}

pub fn is_cache_entry_fresh(cached: &CachedMetadata) -> bool {
    Utc::now() - cached.created_at < Duration::hours(CACHE_TTL_HOURS)
}
