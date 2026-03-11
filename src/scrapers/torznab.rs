use crate::scrapers::{create_magnet, extract_info_hash, parse_size, ScrapedTorrent, Scraper};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use quick_xml::events::{BytesStart, Event};
use quick_xml::reader::Reader;
use reqwest::Client;
use url::Url;

#[derive(Clone, Debug)]
struct TorznabFeed {
    name: String,
    base_url: String,
    api_key: Option<String>,
}

#[derive(Debug, Default)]
struct FeedItem {
    title: String,
    link: String,
    guid: String,
    info_hash: String,
    size_bytes: u64,
    seeders: i32,
    leechers: i32,
    category: String,
}

pub struct TorznabScraper {
    client: Client,
    feeds: Vec<TorznabFeed>,
}

impl TorznabScraper {
    pub fn new() -> Result<Self> {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(8))
            .user_agent("Hydrogene/0.1 (+Torznab)")
            .build()?;

        let feeds = std::env::var("TORZNAB_FEEDS")
            .ok()
            .map(|value| Self::parse_feeds(&value))
            .unwrap_or_default();

        Ok(Self { client, feeds })
    }

    fn parse_feeds(raw: &str) -> Vec<TorznabFeed> {
        raw.split(';')
            .filter_map(|entry| {
                let entry = entry.trim();
                if entry.is_empty() {
                    return None;
                }

                let parts: Vec<_> = entry.split('|').map(str::trim).collect();
                match parts.as_slice() {
                    [name, url] if !name.is_empty() && !url.is_empty() => Some(TorznabFeed {
                        name: (*name).to_string(),
                        base_url: (*url).to_string(),
                        api_key: None,
                    }),
                    [name, url, api_key] if !name.is_empty() && !url.is_empty() => {
                        Some(TorznabFeed {
                            name: (*name).to_string(),
                            base_url: (*url).to_string(),
                            api_key: if api_key.is_empty() {
                                None
                            } else {
                                Some((*api_key).to_string())
                            },
                        })
                    }
                    _ => {
                        tracing::warn!("Skipping invalid TORZNAB_FEEDS entry: {}", entry);
                        None
                    }
                }
            })
            .collect()
    }

    async fn search_feed(
        &self,
        feed: &TorznabFeed,
        query: &str,
        content_type: &str,
    ) -> Result<Vec<ScrapedTorrent>> {
        let mut url = Url::parse(&feed.base_url)
            .map_err(|e| anyhow!("Invalid Torznab URL {}: {}", feed.base_url, e))?;

        {
            let mut pairs = url.query_pairs_mut();
            pairs.append_pair("t", "search");
            pairs.append_pair("q", query);
            pairs.append_pair("cat", Self::categories_for(content_type));
            pairs.append_pair("limit", "100");
            if let Some(api_key) = &feed.api_key {
                pairs.append_pair("apikey", api_key);
            }
        }

        tracing::info!("Torznab search [{}]: {}", feed.name, url);

        let response = self.client.get(url).send().await?;
        if !response.status().is_success() {
            return Err(anyhow!(
                "Torznab {} returned status {}",
                feed.name,
                response.status()
            ));
        }

        let body = response.text().await?;
        self.parse_feed_results(&feed.name, &body)
    }

    fn categories_for(content_type: &str) -> &'static str {
        match content_type {
            "movie" => "2000",
            "series" => "5000,5070",
            _ => "2000,5000,5070",
        }
    }

    fn parse_feed_results(&self, feed_name: &str, xml: &str) -> Result<Vec<ScrapedTorrent>> {
        let mut reader = Reader::from_str(xml);
        reader.trim_text(true);

        let mut torrents = Vec::new();
        let mut current_item = FeedItem::default();
        let mut current_tag = String::new();
        let mut in_item = false;

        loop {
            match reader.read_event() {
                Ok(Event::Start(e)) => {
                    let tag = String::from_utf8_lossy(e.local_name().as_ref()).to_string();
                    if tag == "item" {
                        in_item = true;
                        current_item = FeedItem::default();
                    }
                    if in_item {
                        Self::apply_empty_tag(&mut current_item, &e);
                    }
                    current_tag = tag;
                }
                Ok(Event::Empty(e)) => {
                    if in_item {
                        Self::apply_empty_tag(&mut current_item, &e);
                    }
                }
                Ok(Event::End(e)) => {
                    let tag = String::from_utf8_lossy(e.local_name().as_ref()).to_string();
                    if tag == "item" {
                        if let Some(torrent) = Self::to_scraped_torrent(feed_name, &current_item) {
                            torrents.push(torrent);
                        }
                        in_item = false;
                    }
                    current_tag.clear();
                }
                Ok(Event::Text(e)) => {
                    if in_item {
                        let text = String::from_utf8_lossy(e.as_ref()).to_string();
                        match current_tag.as_str() {
                            "title" => current_item.title = text,
                            "link" => current_item.link = text,
                            "guid" => current_item.guid = text,
                            _ => {}
                        }
                    }
                }
                Ok(Event::Eof) => break,
                Err(e) => {
                    tracing::warn!("Torznab XML parse error for {}: {}", feed_name, e);
                    break;
                }
                _ => {}
            }
        }

        Ok(torrents)
    }

    fn apply_empty_tag(item: &mut FeedItem, e: &BytesStart<'_>) {
        let tag = String::from_utf8_lossy(e.local_name().as_ref()).to_string();

        if tag == "enclosure" {
            for attr in e.attributes().flatten() {
                let key = String::from_utf8_lossy(attr.key.as_ref());
                let value = String::from_utf8_lossy(attr.value.as_ref()).to_string();
                match key.as_ref() {
                    "url" => item.link = value,
                    "length" => item.size_bytes = value.parse::<u64>().unwrap_or(0),
                    _ => {}
                }
            }
            return;
        }

        if tag != "attr" {
            return;
        }

        let mut name = None;
        let mut value = None;
        for attr in e.attributes().flatten() {
            let key = String::from_utf8_lossy(attr.key.as_ref());
            let attr_value = String::from_utf8_lossy(attr.value.as_ref()).to_string();
            match key.as_ref() {
                "name" => name = Some(attr_value),
                "value" => value = Some(attr_value),
                _ => {}
            }
        }

        match (name.as_deref(), value.as_deref()) {
            (Some("seeders"), Some(v)) => item.seeders = v.parse::<i32>().unwrap_or(0),
            (Some("peers"), Some(v)) => item.leechers = v.parse::<i32>().unwrap_or(0),
            (Some("size"), Some(v)) => {
                item.size_bytes = v.parse::<u64>().unwrap_or_else(|_| parse_size(v))
            }
            (Some("category"), Some(v)) => item.category = v.to_string(),
            (Some("infohash"), Some(v)) => item.info_hash = v.to_lowercase(),
            _ => {}
        }
    }

    fn to_scraped_torrent(feed_name: &str, item: &FeedItem) -> Option<ScrapedTorrent> {
        let info_hash = if item.info_hash.is_empty() {
            extract_info_hash(&item.link)
                .or_else(|| extract_info_hash(&item.guid))
                .unwrap_or_default()
        } else {
            item.info_hash.clone()
        };

        if item.title.is_empty() || info_hash.is_empty() {
            return None;
        }

        let magnet_link = if item.link.starts_with("magnet:") {
            item.link.clone()
        } else {
            create_magnet(&info_hash, &item.title)
        };

        Some(ScrapedTorrent {
            title: item.title.clone(),
            info_hash,
            magnet_link,
            size_bytes: item.size_bytes,
            size_gb: item.size_bytes as f64 / 1_073_741_824.0,
            seeders: item.seeders,
            leechers: item.leechers,
            source: feed_name.to_string(),
            category: if item.category.is_empty() {
                "Torznab".to_string()
            } else {
                item.category.clone()
            },
        })
    }
}

#[async_trait]
impl Scraper for TorznabScraper {
    fn name(&self) -> &str {
        "Torznab"
    }

    async fn search(&self, query: &str, content_type: &str) -> Result<Vec<ScrapedTorrent>> {
        if self.feeds.is_empty() {
            return Ok(Vec::new());
        }

        let mut results = Vec::new();
        for feed in &self.feeds {
            match self.search_feed(feed, query, content_type).await {
                Ok(mut torrents) => results.append(&mut torrents),
                Err(e) => tracing::warn!(feed = %feed.name, error = %e, "torznab feed failed"),
            }
        }

        Ok(results)
    }

    fn supports_anime(&self) -> bool {
        true
    }

    fn supports_movies(&self) -> bool {
        true
    }

    fn supports_series(&self) -> bool {
        true
    }
}
