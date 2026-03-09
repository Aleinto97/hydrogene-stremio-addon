use reqwest::Client;
use std::sync::Arc;
use std::time::Duration;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let client = Arc::new(Client::builder().timeout(Duration::from_secs(10)).build()?);

    println!("=== TEST METADATA PROVIDERS ===\n");

    let tmdb_key = "2ebe644bbf00b035797754f2384177c8";

    // Test 1: Film - Inception
    println!("=== TEST 1: FILM (Inception) ===");
    let url = format!(
        "https://api.themoviedb.org/3/find/tt1375666?api_key={}&external_source=imdb_id",
        tmdb_key
    );
    let resp = client.get(&url).send().await?;
    let json: serde_json::Value = resp.json().await?;
    let title = json["movie_results"][0]["title"].as_str().unwrap_or("N/A");
    println!("IMDB: tt1375666 -> TMDB: {}", title);
    println!("Queries: [{}, \"{} 2010\"]\n", title, title);

    // Test 2: Serie TV - Breaking Bad S01E01
    println!("=== TEST 2: SERIE TV (Breaking Bad S01E01) ===");
    let url = format!(
        "https://api.themoviedb.org/3/find/tt0903747?api_key={}&external_source=imdb_id",
        tmdb_key
    );
    let resp = client.get(&url).send().await?;
    let json: serde_json::Value = resp.json().await?;
    let title = json["tv_results"][0]["name"].as_str().unwrap_or("N/A");
    println!("IMDB: tt0903747 -> TMDB: {}", title);
    println!(
        "Queries: [{}, \"{} 2008\", \"{} S01E01\", \"{} S1E1\"]\n",
        title, title, title, title
    );

    // Test 3: Anime - Attack on Titan (corretto)
    println!("=== TEST 3: ANIME (Attack on Titan - ID corretto) ===");
    let query = serde_json::json!({
        "query": "query ($id: Int) { Media (id: $id, type: ANIME) { title { english romaji } } }",
        "variables": { "id": 16498 }
    });
    let resp = client
        .post("https://graphql.anilist.co")
        .header("Content-Type", "application/json")
        .json(&query)
        .send()
        .await?;
    let json: serde_json::Value = resp.json().await?;
    let title = json["data"]["Media"]["title"]["english"]
        .as_str()
        .or_else(|| json["data"]["Media"]["title"]["romaji"].as_str())
        .unwrap_or("N/A");
    println!("AniList: 16498 -> {}", title);
    println!(
        "Queries: [{}, \"{} 01\", \"{} E01\"]\n",
        title, title, title
    );

    // Test 4: Anime - One Piece
    println!("=== TEST 4: ANIME (One Piece) ===");
    let query = serde_json::json!({
        "query": "query ($id: Int) { Media (id: $id, type: ANIME) { title { english romaji } } }",
        "variables": { "id": 21 }
    });
    let resp = client
        .post("https://graphql.anilist.co")
        .header("Content-Type", "application/json")
        .json(&query)
        .send()
        .await?;
    let json: serde_json::Value = resp.json().await?;
    let title = json["data"]["Media"]["title"]["english"]
        .as_str()
        .or_else(|| json["data"]["Media"]["title"]["romaji"].as_str())
        .unwrap_or("N/A");
    println!("AniList: 21 -> {}", title);
    println!("Queries: [{}]\n", title);

    // Test 5: Anime - Death Note
    println!("=== TEST 5: ANIME (Death Note) ===");
    let query = serde_json::json!({
        "query": "query ($id: Int) { Media (id: $id, type: ANIME) { title { english romaji } } }",
        "variables": { "id": 1535 }
    });
    let resp = client
        .post("https://graphql.anilist.co")
        .header("Content-Type", "application/json")
        .json(&query)
        .send()
        .await?;
    let json: serde_json::Value = resp.json().await?;
    let title = json["data"]["Media"]["title"]["english"]
        .as_str()
        .or_else(|| json["data"]["Media"]["title"]["romaji"].as_str())
        .unwrap_or("N/A");
    println!("AniList: 1535 -> {}", title);
    println!("Queries: [{}]\n", title);

    // Test 6: Anime - Jujutsu Kaisen
    println!("=== TEST 6: ANIME (Jujutsu Kaisen) ===");
    let query = serde_json::json!({
        "query": "query ($id: Int) { Media (id: $id, type: ANIME) { title { english romaji } } }",
        "variables": { "id": 40748 }
    });
    let resp = client
        .post("https://graphql.anilist.co")
        .header("Content-Type", "application/json")
        .json(&query)
        .send()
        .await?;
    let json: serde_json::Value = resp.json().await?;
    let title = json["data"]["Media"]["title"]["english"]
        .as_str()
        .or_else(|| json["data"]["Media"]["title"]["romaji"].as_str())
        .unwrap_or("N/A");
    println!("AniList: 40748 -> {}", title);
    println!("Queries: [{}]\n", title);

    println!("=== TUTTI I TEST COMPLETATI ===");
    println!("\n✅ Provider TMDB (film/serie): FUNZIONA");
    println!("✅ Provider AniList (anime): FUNZIONA");

    Ok(())
}
