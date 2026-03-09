use once_cell::sync::Lazy;
use regex::Regex;
use std::fmt::Display;

#[derive(Debug, Clone)]
pub enum CacheKey {
    Movie {
        imdb_id: String,
    },
    Series {
        imdb_id: String,
        season: u32,
        episode: u32,
    },
}

impl Display for CacheKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CacheKey::Movie { imdb_id } => write!(f, "movie:{}", imdb_id),
            CacheKey::Series {
                imdb_id,
                season,
                episode,
            } => write!(f, "series:{}:{}:{}", imdb_id, season, episode),
        }
    }
}

impl CacheKey {
    pub fn from_stremio_id(id: &str) -> Option<Self> {
        let parts: Vec<&str> = id.split(':').collect();

        if parts[0].starts_with("tt") {
            if parts.len() >= 3 {
                let season = parts[1].parse::<u32>().ok()?;
                let episode = parts[2].parse::<u32>().ok()?;
                Some(CacheKey::Series {
                    imdb_id: parts[0].to_string(),
                    season,
                    episode,
                })
            } else {
                Some(CacheKey::Movie {
                    imdb_id: parts[0].to_string(),
                })
            }
        } else if parts[0] == "anilist" {
            if parts.len() >= 3 {
                let episode = parts[2].parse::<u32>().ok()?;
                Some(CacheKey::Series {
                    imdb_id: id.to_string(),
                    season: 1,
                    episode,
                })
            } else {
                Some(CacheKey::Series {
                    imdb_id: id.to_string(),
                    season: 1,
                    episode: 0,
                })
            }
        } else {
            None
        }
    }

    pub fn as_str(&self) -> String {
        self.to_string()
    }
}

static EPISODE_REGEX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)\bS(\d{1,2})E(\d{1,3})\b").expect("Invalid regex pattern"));

static MULTI_EPISODE_REGEX: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\bS(\d{1,2})E(\d{1,3})-(?:E)?(\d{1,3})\b").expect("Invalid regex pattern")
});

static ANIME_EPISODE_REGEX: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)(?:^|[\s\[\]\-_])(?:E|EP|Episode)?\s*(\d{1,3})(?:v\d)?(?:[\s\[\]\-_]|$|\.mkv|\.mp4|\.avi)").expect("Invalid regex pattern")
});

static ANIME_BRACKET_REGEX: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\[(?:[^\]]*\s+)?(\d{1,3})(?:v\d)?(?:\s+[^\]]*)?\]")
        .expect("Invalid regex pattern")
});

static SEASON_ONLY_REGEX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)\b(?:S|SEASON\s*)(\d{1,2})\b").expect("Invalid regex pattern"));

static EPISODE_MARKER_REGEX: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\b(?:E|EP|EPISODE| -)\s*(\d{1,3})\b").expect("Invalid regex pattern")
});

pub fn is_exact_episode_match(title: &str, season: u32, episode: u32) -> bool {
    let title_upper = title.to_uppercase();

    if let Some(caps) = MULTI_EPISODE_REGEX.captures(&title_upper) {
        let matched_season = caps.get(1).and_then(|m| m.as_str().parse::<u32>().ok());
        let start_ep = caps.get(2).and_then(|m| m.as_str().parse::<u32>().ok());
        let end_ep = caps.get(3).and_then(|m| m.as_str().parse::<u32>().ok());

        if matched_season == Some(season) {
            if let (Some(start), Some(end)) = (start_ep, end_ep) {
                if episode >= start && episode <= end {
                    return true;
                }
            }
        }
    }

    if let Some(_caps) = EPISODE_REGEX.captures(&title_upper) {
        let mut matched_any = false;
        let mut matched_wrong = false;

        for caps in EPISODE_REGEX.captures_iter(&title_upper) {
            let matched_season = caps.get(1).and_then(|m| m.as_str().parse::<u32>().ok());
            let matched_episode = caps.get(2).and_then(|m| m.as_str().parse::<u32>().ok());

            if matched_season == Some(season) && matched_episode == Some(episode) {
                matched_any = true;
            } else if matched_season == Some(season) {
                matched_wrong = true;
            }
        }

        if matched_any {
            return true;
        }
        if matched_wrong {
            return false;
        }
    }

    let mut is_season_match = false;
    for caps in SEASON_ONLY_REGEX.captures_iter(&title_upper) {
        let matched_season = caps.get(1).and_then(|m| m.as_str().parse::<u32>().ok());
        if matched_season == Some(season) {
            is_season_match = true;
            break;
        }
    }

    if is_season_match {
        if let Some(caps) = EPISODE_MARKER_REGEX.captures(&title_upper) {
            let matched_ep = caps.get(1).and_then(|m| m.as_str().parse::<u32>().ok());
            if matched_ep != Some(episode) {
                return false;
            }
        }
        return true;
    }

    if season == 1 || season == 0 {
        for caps in ANIME_EPISODE_REGEX.captures_iter(&title_upper) {
            let matched_ep = caps.get(1).and_then(|m| m.as_str().parse::<u32>().ok());
            if matched_ep == Some(episode) {
                return true;
            }
        }

        for caps in ANIME_BRACKET_REGEX.captures_iter(&title_upper) {
            let matched_ep = caps.get(1).and_then(|m| m.as_str().parse::<u32>().ok());
            if matched_ep == Some(episode) {
                return true;
            }
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_key_display() {
        let movie_key = CacheKey::Movie {
            imdb_id: "tt1234".to_string(),
        };
        assert_eq!(movie_key.to_string(), "movie:tt1234");

        let series_key = CacheKey::Series {
            imdb_id: "tt5678".to_string(),
            season: 2,
            episode: 5,
        };
        assert_eq!(series_key.to_string(), "series:tt5678:2:5");
    }

    #[test]
    fn test_is_exact_episode_match() {
        assert!(is_exact_episode_match("Show S01E02 1080p", 1, 2));
        assert!(is_exact_episode_match("Show.S01E02.1080p.BluRay", 1, 2));
        assert!(is_exact_episode_match("Show s01e02 1080p", 1, 2));
        assert!(!is_exact_episode_match("Show S01E02 1080p", 2, 2));
        assert!(!is_exact_episode_match("Show S01E02 1080p", 1, 3));
        assert!(!is_exact_episode_match("Show S01E016 1080p", 1, 1));

        assert!(is_exact_episode_match("Show S01E01-E03 1080p", 1, 2));
        assert!(is_exact_episode_match("Show S01E01-E03 1080p", 1, 1));
        assert!(is_exact_episode_match("Show S01E01-E03 1080p", 1, 3));
        assert!(!is_exact_episode_match("Show S01E01-E03 1080p", 1, 4));

        assert!(is_exact_episode_match("[Group] Anime - 05 [1080p]", 1, 5));
        assert!(is_exact_episode_match("[Group] Anime 05 [1080p]", 1, 5));
        assert!(is_exact_episode_match("Anime Episode 12 1080p", 1, 12));
        assert!(is_exact_episode_match("Anime EP03 1080p", 1, 3));
    }

    #[test]
    fn test_cache_key_from_stremio_id() {
        let movie = CacheKey::from_stremio_id("tt1234567").unwrap();
        assert!(matches!(movie, CacheKey::Movie { imdb_id } if imdb_id == "tt1234567"));

        let series = CacheKey::from_stremio_id("tt1234567:3:12").unwrap();
        assert!(
            matches!(series, CacheKey::Series { imdb_id, season, episode } 
            if imdb_id == "tt1234567" && season == 3 && episode == 12)
        );

        let anime = CacheKey::from_stremio_id("anilist:12345:5").unwrap();
        assert!(
            matches!(anime, CacheKey::Series { imdb_id, season, episode } 
            if imdb_id == "anilist:12345:5" && season == 1 && episode == 5)
        );

        assert!(CacheKey::from_stremio_id("invalid").is_none());
    }
}
