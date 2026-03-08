use once_cell::sync::Lazy;
use regex::Regex;
use std::fmt::Display;

/// Regex type-safe cache key enum
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
    /// Create a CacheKey from a Stremio ID string
    /// Handles formats like: "tt1234567", "tt1234567:1:3", "anilist:12345:1"
    pub fn from_stremio_id(id: &str) -> Option<Self> {
        let parts: Vec<&str> = id.split(':').collect();

        if parts[0].starts_with("tt") {
            // IMDB ID format
            if parts.len() >= 3 {
                // Series: tt1234567:1:3
                let season = parts[1].parse::<u32>().ok()?;
                let episode = parts[2].parse::<u32>().ok()?;
                Some(CacheKey::Series {
                    imdb_id: parts[0].to_string(),
                    season,
                    episode,
                })
            } else {
                // Movie: tt1234567
                Some(CacheKey::Movie {
                    imdb_id: parts[0].to_string(),
                })
            }
        } else if parts[0] == "anilist" {
            // Anime IDs - treat as series with optional episode
            if parts.len() >= 3 {
                let episode = parts[2].parse::<u32>().ok()?;
                Some(CacheKey::Series {
                    imdb_id: id.to_string(),
                    season: 1, // Anime typically doesn't have seasons
                    episode,
                })
            } else {
                // No episode specified, treat as series request (all episodes)
                Some(CacheKey::Series {
                    imdb_id: id.to_string(),
                    season: 1,
                    episode: 0, // 0 means all episodes
                })
            }
        } else {
            None
        }
    }

    /// Get the string representation for cache operations
    pub fn as_str(&self) -> String {
        self.to_string()
    }
}

/// Regex pattern for exact season/episode matching with word boundaries
/// Compiled once for performance using lazy_static pattern
static EPISODE_REGEX: Lazy<Regex> = Lazy::new(|| {
    // (?i) = case-insensitive flag
    // \b = word boundary (prevents matching partial strings like S01E016)
    Regex::new(r"(?i)\bS(\d{1,2})E(\d{1,3})\b").expect("Invalid regex pattern")
});

/// Check if a title matches exactly the specified season and episode
/// Uses word boundaries to avoid false positives (e.g., "S01E01" should NOT match "S01E016")
///
/// # Examples
/// ```
/// use your_crate::utils::is_exact_episode_match;
///
/// assert!(is_exact_episode_match("Show S01E02 1080p", 1, 2));
/// assert!(!is_exact_episode_match("Show S01E02 1080p", 1, 3)); // Wrong episode
/// assert!(!is_exact_episode_match("Show S01E016 1080p", 1, 1)); // S01E016 != S01E01
/// assert!(is_exact_episode_match("Show s01e02 1080p", 1, 2)); // Case-insensitive
/// ```
pub fn is_exact_episode_match(title: &str, season: u32, episode: u32) -> bool {
    EPISODE_REGEX.captures_iter(title).any(|caps| {
        let matched_season = caps.get(1).and_then(|m| m.as_str().parse::<u32>().ok());
        let matched_episode = caps.get(2).and_then(|m| m.as_str().parse::<u32>().ok());

        matched_season == Some(season) && matched_episode == Some(episode)
    })
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
        // Exact match
        assert!(is_exact_episode_match("Show S01E02 1080p", 1, 2));
        assert!(is_exact_episode_match("Show.S01E02.1080p.BluRay", 1, 2));

        // Case insensitive
        assert!(is_exact_episode_match("Show s01e02 1080p", 1, 2));
        assert!(is_exact_episode_match("Show S01e02 1080p", 1, 2));

        // Wrong season or episode
        assert!(!is_exact_episode_match("Show S01E02 1080p", 2, 2));
        assert!(!is_exact_episode_match("Show S01E02 1080p", 1, 3));

        // Word boundaries prevent partial matches (S01E01 vs S01E016)
        assert!(!is_exact_episode_match("Show S01E016 1080p", 1, 1));
        assert!(!is_exact_episode_match("Show S01E01Extra 1080p", 1, 1));
        assert!(!is_exact_episode_match("Show XXS01E01YY 1080p", 1, 1));

        // Correct match even with numbers nearby
        assert!(is_exact_episode_match("Show S01E01 S02E02", 1, 1));
        assert!(is_exact_episode_match("Show S01E01 S02E02", 2, 2));
    }

    #[test]
    fn test_cache_key_from_stremio_id() {
        // Movie
        let movie = CacheKey::from_stremio_id("tt1234567").unwrap();
        assert!(matches!(movie, CacheKey::Movie { imdb_id } if imdb_id == "tt1234567"));

        // Series
        let series = CacheKey::from_stremio_id("tt1234567:3:12").unwrap();
        assert!(
            matches!(series, CacheKey::Series { imdb_id, season, episode } 
            if imdb_id == "tt1234567" && season == 3 && episode == 12)
        );

        // Anime
        let anime = CacheKey::from_stremio_id("anilist:12345:5").unwrap();
        assert!(
            matches!(anime, CacheKey::Series { imdb_id, season, episode } 
            if imdb_id == "anilist:12345:5" && season == 1 && episode == 5)
        );

        // Invalid
        assert!(CacheKey::from_stremio_id("invalid").is_none());
    }
}
