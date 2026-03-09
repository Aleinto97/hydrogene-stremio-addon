use sqlx::postgres::PgPoolOptions;
use std::time::Duration;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let database_url = "postgresql://postgres:zikceq-cignap-Bukwa1@db.sfvnyokacmznwfsncnxm.supabase.co:5432/postgres?sslmode=require";

    let pool = PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_secs(10))
        .connect(database_url)
        .await?;

    println!("✅ Connected to database");

    // Get table names
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT table_name FROM information_schema.tables WHERE table_schema = 'public'",
    )
    .fetch_all(&pool)
    .await?;

    println!("Tables in database:");
    for (table,) in &rows {
        println!("  - {}", table);
    }

    // Delete from cached_torrents
    let deleted = sqlx::query("DELETE FROM cached_torrents")
        .execute(&pool)
        .await?;
    println!(
        "\n🗑️ Deleted {} rows from cached_torrents",
        deleted.rows_affected()
    );

    // Delete from cached_resolves
    let deleted = sqlx::query("DELETE FROM cached_resolves")
        .execute(&pool)
        .await?;
    println!(
        "🗑️ Deleted {} rows from cached_resolves",
        deleted.rows_affected()
    );

    println!("\n✅ Database cleared!");

    Ok(())
}
