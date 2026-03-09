use reqwest::Client;
use serde::Deserialize;
use anyhow::{Result, anyhow};
use tracing::{info, warn};

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
    #[serde(rename = "mimeType")]
    pub mime_type: String,
    link: String,
    host: String,
    chunks: i64,
    crc: i64,
    download: String,
    streamable: i64,
}

#[derive(Debug)]
pub enum ResolveResult {
    Ready(String),      // URL video pronto
    Downloading(f64),   // In download, progresso percentuale
    Queued,             // In coda
    Processing,         // Elaborazione metadati
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

    pub async fn resolve_magnet(&self, info_hash: &str, season: Option<u32>, episode: Option<u32>) -> Result<String> {
        let magnet = format!("magnet:?xt=urn:btih:{}", info_hash);
        
        info!("Adding magnet to Real-Debrid: {}", info_hash);
        
        // Step 1: Add magnet
        let torrent_id = self.add_magnet(&magnet).await?;
        info!("Magnet added, torrent ID: {}", torrent_id);
        
        // Step 2: Get torrent info and wait for metadata (max 60 seconds)
        let torrent_info = self.wait_for_metadata(&torrent_id).await?;
        info!("Torrent metadata received: {} files", torrent_info.files.len());
        
        // Step 3: Select the best video file
        let main_video_id = self.select_best_file(&torrent_info, season, episode)?;
        info!("Selected best video file: ID {}", main_video_id);
        
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

    pub async fn resolve_magnet_with_status(
        &self, 
        info_hash: &str, 
        season: Option<u32>, 
        episode: Option<u32>
    ) -> Result<ResolveResult> {
        let magnet = format!("magnet:?xt=urn:btih:{}", info_hash);
        
        info!("Adding magnet to Real-Debrid: {}", info_hash);
        
        // Step 1: Add magnet
        let torrent_id = self.add_magnet(&magnet).await?;
        info!("Magnet added, torrent ID: {}", torrent_id);
        
        // Step 2: Get torrent info immediately (no wait) to check status
        let torrent_info = self.get_torrent_info(&torrent_id).await?;
        info!("Initial torrent status: {}", torrent_info.status);
        
        // Check if already downloaded (cached)
        if torrent_info.status == "downloaded" {
            info!("Torrent is already cached/downloaded");
            // Select and unrestrict immediately
            let main_video_id = self.select_best_file(&torrent_info, season, episode)?;
            self.select_files(&torrent_id, &main_video_id.to_string()).await?;
            
            // Re-fetch to get links
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
            let updated_info = self.get_torrent_info(&torrent_id).await?;
            
            if let Some(link) = updated_info.links.first() {
                let video_url = self.unrestrict_link(link).await?;
                return Ok(ResolveResult::Ready(video_url));
            } else {
                return Err(anyhow!("No links available in cached torrent after selection"));
            }
        }
        
        // Step 3: Wait for metadata if needed
        let mut torrent_info = self.wait_for_metadata(&torrent_id).await?;
        
        // If we have files but haven't selected them yet, select them now
        if torrent_info.status == "waiting_files_selection" && !torrent_info.files.is_empty() {
            info!("Torrent needs file selection, selecting main video...");
            let main_video_id = self.select_best_file(&torrent_info, season, episode)?;
            self.select_files(&torrent_id, &main_video_id.to_string()).await?;
            
            // Get updated info after selection
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
            torrent_info = self.get_torrent_info(&torrent_id).await?;
            info!("Status after file selection: {}", torrent_info.status);
        }
        
        // Check status after metadata and potential selection
        match torrent_info.status.as_str() {
            "downloaded" => {
                info!("Torrent downloaded/cached");
                
                // Ensure files are selected (in case it was already downloaded but not selected)
                if torrent_info.links.is_empty() {
                    let main_video_id = self.select_best_file(&torrent_info, season, episode)?;
                    self.select_files(&torrent_id, &main_video_id.to_string()).await?;
                    // Re-fetch to get links
                    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                    let updated_info = self.get_torrent_info(&torrent_id).await?;
                    if let Some(link) = updated_info.links.first() {
                        let video_url = self.unrestrict_link(link).await?;
                        return Ok(ResolveResult::Ready(video_url));
                    }
                } else if let Some(link) = torrent_info.links.first() {
                    let video_url = self.unrestrict_link(link).await?;
                    return Ok(ResolveResult::Ready(video_url));
                }
                
                Err(anyhow!("No links available after download/selection"))
            }
            "downloading" => {
                info!("Torrent is downloading: {}%", torrent_info.progress);
                Ok(ResolveResult::Downloading(torrent_info.progress))
            }
            "queued" => {
                info!("Torrent is queued for download");
                Ok(ResolveResult::Queued)
            }
            "magnet_conversion" | "waiting_files_selection" => {
                info!("Torrent is still processing metadata or waiting selection");
                Ok(ResolveResult::Processing)
            }
            "error" => {
                Err(anyhow!("Torrent error on RD"))
            }
            "virus" => {
                Err(anyhow!("Torrent flagged as virus by RD"))
            }
            "dead" => {
                Err(anyhow!("Torrent is dead (no seeds)"))
            }
            _ => {
                // For other statuses, try to wait a bit
                info!("Unknown status: {}, attempting to wait...", torrent_info.status);
                let check_interval = 5;
                let max_checks = 12; // 1 minute max for this loop
                
                for check in 0..max_checks {
                    tokio::time::sleep(tokio::time::Duration::from_secs(check_interval)).await;
                    let info = self.get_torrent_info(&torrent_id).await?;
                    
                    match info.status.as_str() {
                        "downloaded" => {
                            info!("Torrent ready after {} checks", check + 1);
                            if let Some(link) = info.links.first() {
                                let video_url = self.unrestrict_link(link).await?;
                                return Ok(ResolveResult::Ready(video_url));
                            }
                        }
                        "downloading" => {
                            return Ok(ResolveResult::Downloading(info.progress));
                        }
                        "error" | "virus" | "dead" => {
                            return Err(anyhow!("Torrent error during background wait: {}", info.status));
                        }
                        _ => continue,
                    }
                }
                
                // Timeout - still not ready
                Ok(ResolveResult::Downloading(torrent_info.progress))
            }
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

    fn select_best_file(&self, info: &RDTorrentInfo, season: Option<u32>, episode: Option<u32>) -> Result<i64> {
        let video_extensions = [
            ".mp4", ".mkv", ".avi", ".mov", ".webm", ".m4v", ".ts", ".mts", ".m2ts",
            ".mpg", ".mpeg", ".wmv", ".flv", ".f4v", ".3gp", ".3g2", ".ogv", ".ogm"
        ];
        
        // Keywords to exclude (sample files, extras, etc.)
        let exclude_keywords = ["sample", "trailer", "extra", "featurette", "bonus", "behindthescenes"];
        
        let mut videos: Vec<&RDFile> = info.files
            .iter()
            .filter(|f| {
                let path_lower = f.path.to_lowercase();
                // Check if it's a video file
                let is_video = video_extensions.iter().any(|ext| path_lower.ends_with(ext));
                // Check if it's not a sample/extra file
                let is_not_sample = !exclude_keywords.iter().any(|kw| path_lower.contains(kw));
                is_video && is_not_sample
            })
            .collect();

        if videos.is_empty() {
            return Err(anyhow!("No video files found in torrent"));
        }

        // --- EPISODE SELECTION LOGIC ---
        if let (Some(s), Some(e)) = (season, episode) {
            info!("Looking for S{:02}E{:02} in torrent files...", s, e);
            
            // Try to find exact episode match in path
            let best_match = videos.iter().find(|v| {
                crate::utils::is_exact_episode_match(&v.path, s, e)
            });
            
            if let Some(found) = best_match {
                info!("Found matching episode file: {}", found.path);
                return Ok(found.id);
            }
            
            warn!("Episode S{:02}E{:02} not explicitly found in files, falling back to largest file", s, e);
        }

        // Sort by size, pick largest (main video is usually the biggest)
        videos.sort_by(|a, b| b.bytes.cmp(&a.bytes));
        
        let selected = videos.first().unwrap();
        info!("Selected video file: {} ({} bytes)", selected.path, selected.bytes);
        
        Ok(selected.id)
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