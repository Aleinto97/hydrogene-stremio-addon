use reqwest::Client;
use serde::{Deserialize, Serialize};
use anyhow::{Result, anyhow};
use tracing::{info, error, debug};

const RD_API_BASE: &str = "https://api.real-debrid.com/rest/1.0";

pub struct RealDebridClient {
    client: Client,
    api_key: String,
}

#[derive(Debug, Deserialize)]
struct RDTorrentResponse {
    id: String,
    uri: String,
}

#[derive(Debug, Deserialize)]
struct RDTorrentInfo {
    id: String,
    filename: String,
    original_filename: String,
    hash: String,
    bytes: i64,
    original_bytes: i64,
    host: String,
    split: i64,
    progress: f64,
    status: String,
    added: String,
    files: Vec<RDFile>,
    links: Vec<String>,
    ended: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RDFile {
    id: i64,
    path: String,
    bytes: i64,
    selected: i64,
}

#[derive(Debug, Deserialize)]
struct RDUnrestrictResponse {
    id: String,
    filename: String,
    mimeType: String,
    link: String,
    host: String,
    chunks: i64,
    crc: i64,
    download: String,
    streamable: i64,
}

impl RealDebridClient {
    pub fn new() -> Result<Self> {
        let api_key = std::env::var("RD_API_KEY")
            .map_err(|_| anyhow!("RD_API_KEY environment variable not set"))?;

        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()?;

        Ok(Self { client, api_key })
    }

    pub async fn resolve_magnet(&self, info_hash: &str) -> Result<String> {
        let magnet = format!("magnet:?xt=urn:btih:{}", info_hash);
        
        info!("Adding magnet to Real-Debrid: {}", info_hash);
        
        // Step 1: Add magnet
        let torrent_id = self.add_magnet(&magnet).await?;
        info!("Magnet added, torrent ID: {}", torrent_id);
        
        // Step 2: Get torrent info and wait for metadata (max 60 seconds)
        let torrent_info = self.wait_for_metadata(&torrent_id).await?;
        info!("Torrent metadata received: {} files", torrent_info.files.len());
        
        // Step 3: Select the main video file
        let main_video_id = self.select_main_video(&torrent_info)?;
        info!("Selected main video file: ID {}", main_video_id);
        
        self.select_files(&torrent_id, &main_video_id.to_string()).await?;
        
        // Step 4: Wait for download to complete (no timeout limit)
        info!("Waiting for download to complete...");
        let completed_info = self.wait_for_download(&torrent_id).await?;
        info!("Torrent ready, {} links available", completed_info.links.len());
        
        // Step 5: Unrestrict the link
        if let Some(link) = completed_info.links.first() {
            let video_url = self.unrestrict_link(link).await?;
            info!("Got direct video URL");
            Ok(video_url)
        } else {
            Err(anyhow!("No links available after download"))
        }
    }

    async fn add_magnet(&self, magnet: &str) -> Result<String> {
        let url = format!("{}/torrents/addMagnet", RD_API_BASE);
        
        let form = [("magnet", magnet)];
        
        let response = self.client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .form(&form)
            .send()
            .await?;

        if !response.status().is_success() {
            let text = response.text().await?;
            return Err(anyhow!("Failed to add magnet: {}", text));
        }

        let result: RDTorrentResponse = response.json().await?;
        Ok(result.id)
    }

    async fn get_torrent_info(&self, torrent_id: &str) -> Result<RDTorrentInfo> {
        let url = format!("{}/torrents/info/{}", RD_API_BASE, torrent_id);
        
        let response = self.client
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .send()
            .await?;

        if !response.status().is_success() {
            let text = response.text().await?;
            return Err(anyhow!("Failed to get torrent info: {}", text));
        }

        let info: RDTorrentInfo = response.json().await?;
        Ok(info)
    }

    async fn wait_for_metadata(&self, torrent_id: &str) -> Result<RDTorrentInfo> {
        let mut attempts = 0;
        let max_attempts = 30; // 30 seconds max

        loop {
            let info = self.get_torrent_info(torrent_id).await?;
            
            match info.status.as_str() {
                "magnet_conversion" | "waiting_files_selection" | "queued" => {
                    if info.files.is_empty() {
                        attempts += 1;
                        if attempts >= max_attempts {
                            return Err(anyhow!("Timeout waiting for metadata"));
                        }
                        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                        continue;
                    }
                    return Ok(info);
                }
                "downloading" | "uploading" | "compressing" => {
                    return Ok(info);
                }
                "downloaded" => {
                    return Ok(info);
                }
                "error" => {
                    return Err(anyhow!("Torrent error"));
                }
                _ => {
                    attempts += 1;
                    if attempts >= max_attempts {
                        return Err(anyhow!("Timeout waiting for metadata"));
                    }
                    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                }
            }
        }
    }

    fn select_main_video(&self, info: &RDTorrentInfo) -> Result<i64> {
        let video_extensions = [".mp4", ".mkv", ".avi", ".mov", ".webm", ".m4v"];
        
        let mut videos: Vec<&RDFile> = info.files
            .iter()
            .filter(|f| {
                let path_lower = f.path.to_lowercase();
                video_extensions.iter().any(|ext| path_lower.ends_with(ext))
            })
            .collect();

        if videos.is_empty() {
            // If no video extensions found, try to find largest file
            videos = info.files.iter().collect();
        }

        // Sort by size, pick largest
        videos.sort_by(|a, b| b.bytes.cmp(&a.bytes));
        
        videos.first()
            .map(|f| f.id)
            .ok_or_else(|| anyhow!("No files found in torrent"))
    }

    async fn select_files(&self, torrent_id: &str, file_id: &str) -> Result<()> {
        let url = format!("{}/torrents/selectFiles/{}", RD_API_BASE, torrent_id);
        
        let form = [("files", file_id)];
        
        let response = self.client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .form(&form)
            .send()
            .await?;

        if !response.status().is_success() {
            let text = response.text().await?;
            return Err(anyhow!("Failed to select files: {}", text));
        }

        Ok(())
    }

    async fn wait_for_download(&self, torrent_id: &str) -> Result<RDTorrentInfo> {
        let mut attempts = 0;
        let check_interval = 5; // Check every 5 seconds
        
        loop {
            let info = self.get_torrent_info(torrent_id).await?;
            
            match info.status.as_str() {
                "downloaded" => {
                    return Ok(info);
                }
                "error" => {
                    return Err(anyhow!("Torrent download error"));
                }
                _ => {
                    attempts += 1;
                    let progress = info.progress;
                    let status = &info.status;
                    info!("Torrent {} status: {} ({}% complete, attempt {})", 
                          torrent_id, status, progress, attempts);
                    
                    // Check every 5 seconds regardless of status
                    tokio::time::sleep(tokio::time::Duration::from_secs(check_interval)).await;
                }
            }
        }
    }

    async fn unrestrict_link(&self, link: &str) -> Result<String> {
        let url = format!("{}/unrestrict/link", RD_API_BASE);
        
        let form = [("link", link)];
        
        let response = self.client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .form(&form)
            .send()
            .await?;

        if !response.status().is_success() {
            let text = response.text().await?;
            return Err(anyhow!("Failed to unrestrict link: {}", text));
        }

        let result: RDUnrestrictResponse = response.json().await?;
        Ok(result.download)
    }
}