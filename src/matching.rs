use once_cell::sync::Lazy;
use regex::Regex;

use crate::utils::is_exact_episode_match;

static YEAR_REGEX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)\b((?:19|20)\d{2})\b").expect("Invalid year regex"));

static SEASON_REGEX: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\b(?:S|SEASON\s*)(\d{1,2})(?:E\d{1,3}\b|\b)").expect("Invalid season regex")
});

static EPISODE_REGEX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)\bS(\d{1,2})E(\d{1,3})\b").expect("Invalid episode regex"));

static COMPLETE_SEASON_REGEX: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?i)\b(?:complete|season\s*\d{1,2}\s*complete|complete\s*season|全集|batch|pack)\b",
    )
    .expect("Invalid complete season regex")
});

static ANIME_EP_MARKER_REGEX: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\b(?:E|EP|EPISODE)\s*\d{1,3}\b").expect("Invalid anime episode regex")
});

#[derive(Debug, Clone)]
pub struct TorrentMatchScore {
    pub title_match: f64,
    pub season_score: i32,
    pub episode_score: i32,
    pub year_score: i32,
    pub quality_score: i32,
    pub total: i32,
}

impl TorrentMatchScore {
    pub fn new() -> Self {
        Self {
            title_match: 0.0,
            season_score: 0,
            episode_score: 0,
            year_score: 0,
            quality_score: 0,
            total: 0,
        }
    }

    pub fn calculate(&mut self) -> i32 {
        let mut score = 0;

        // Title matching (fuzzy) - up to 40 points
        score += (self.title_match * 40.0).min(40.0) as i32;

        score += self.season_score;
        score += self.episode_score;
        score += self.year_score;

        // Quality score - up to 50 points
        score += self.quality_score.min(50);

        self.total = score;
        score
    }
}

pub fn fuzzy_match_title(query: &str, title: &str) -> f64 {
    let query_clean = clean_title(query);
    let title_clean = clean_title(title);

    // Exact match
    if query_clean == title_clean {
        return 1.0;
    }

    // Similarity using strsim
    let similarity = strsim::jaro_winkler(&query_clean, &title_clean);

    // Check if title contains query (partial match)
    if title_clean.contains(&query_clean) {
        return 0.8 + (similarity * 0.2);
    }

    similarity
}

pub fn has_required_title_tokens(query: &str, title: &str) -> bool {
    let query_tokens = significant_tokens(query);
    if query_tokens.is_empty() {
        return true;
    }

    let title_tokens = significant_tokens(title);
    if title_tokens.is_empty() {
        return false;
    }

    for token in query_tokens.iter().filter(|token| is_numeric_token(token)) {
        if !title_tokens.iter().any(|candidate| candidate == token) {
            return false;
        }
    }

    let matched = query_tokens
        .iter()
        .filter(|token| title_tokens.iter().any(|candidate| candidate == *token))
        .count();

    matched * 2 >= query_tokens.len().max(2)
}

fn clean_title(title: &str) -> String {
    title
        .to_lowercase()
        .replace(&['.', '-', '_', '[', ']', '(', ')'][..], " ")
        .replace(':', " ")
        .split_whitespace()
        .filter(|token| !is_noise_token(token))
        .collect::<Vec<&str>>()
        .join(" ")
}

fn significant_tokens(title: &str) -> Vec<String> {
    clean_title(title)
        .split_whitespace()
        .filter(|token| !is_stopword(token))
        .map(str::to_string)
        .collect()
}

fn is_noise_token(token: &str) -> bool {
    matches!(
        token,
        "2160p"
            | "1080p"
            | "720p"
            | "480p"
            | "4k"
            | "uhd"
            | "hdr"
            | "webrip"
            | "web"
            | "webdl"
            | "web-dl"
            | "bluray"
            | "blu-ray"
            | "bdrip"
            | "x264"
            | "x265"
            | "h264"
            | "h265"
            | "hevc"
            | "av1"
            | "aac"
            | "ddp5"
            | "ddp5.1"
            | "dd5.1"
            | "ac3"
            | "dts"
            | "remux"
            | "proper"
            | "repack"
    ) || token.starts_with('s')
        && token.contains('e')
        && token[1..].chars().any(|c| c.is_ascii_digit())
}

fn is_stopword(token: &str) -> bool {
    matches!(
        token,
        "the" | "a" | "an" | "and" | "of" | "to" | "in" | "on" | "for" | "with"
    )
}

fn is_numeric_token(token: &str) -> bool {
    !token.is_empty() && token.chars().all(|c| c.is_ascii_digit())
}

pub fn extract_year(title: &str) -> Option<u32> {
    extract_all_years(title).into_iter().next()
}

pub fn extract_all_years(title: &str) -> Vec<u32> {
    YEAR_REGEX
        .captures_iter(&title.to_uppercase())
        .filter_map(|caps| caps.get(1).and_then(|m| m.as_str().parse().ok()))
        .collect()
}

pub fn extract_season(title: &str) -> Option<u32> {
    SEASON_REGEX
        .captures_iter(&title.to_uppercase())
        .filter_map(|caps| caps.get(1).and_then(|m| m.as_str().parse().ok()))
        .next()
}

pub fn extract_episode(title: &str) -> Option<u32> {
    EPISODE_REGEX
        .captures_iter(&title.to_uppercase())
        .filter_map(|caps| caps.get(2).and_then(|m| m.as_str().parse().ok()))
        .next()
}

pub fn calculate_match_score(
    query_title: &str,
    query_year: Option<u32>,
    query_season: Option<u32>,
    query_episode: Option<u32>,
    torrent_title: &str,
    _seeders: i32,
    _size_bytes: u64,
) -> i32 {
    let mut score = TorrentMatchScore::new();

    // Title matching
    score.title_match = fuzzy_match_title(query_title, torrent_title);

    // Year matching
    if let Some(target_year) = query_year {
        if let Some(torrent_year) = extract_year(torrent_title) {
            score.year_score = if target_year == torrent_year { 25 } else { -35 };
        }
    }

    // Season matching
    if let Some(target_season) = query_season {
        if let Some(torrent_season) = extract_season(torrent_title) {
            score.season_score = if target_season == torrent_season {
                25
            } else {
                -40
            };
        }
    }

    // Episode matching: exact episode first, season packs as fallback only.
    if let Some(target_season) = query_season {
        if let Some(target_episode) = query_episode {
            if is_exact_episode_match(torrent_title, target_season, target_episode) {
                score.episode_score = 40;
            } else if is_likely_season_pack(torrent_title, target_season) {
                score.episode_score = 10;
            } else if has_conflicting_episode(torrent_title, target_season, target_episode) {
                score.episode_score = -50;
            }
        }
    }

    // Quality scoring
    let title_upper = torrent_title.to_uppercase();
    if title_upper.contains("2160P") || title_upper.contains("4K") || title_upper.contains("UHD") {
        score.quality_score += 100;
    } else if title_upper.contains("1080P") {
        score.quality_score += 80;
    } else if title_upper.contains("720P") {
        score.quality_score += 60;
    }

    if title_upper.contains("BLURAY") || title_upper.contains("BDRIP") {
        score.quality_score += 30;
    }

    score.calculate()
}

fn is_likely_season_pack(title: &str, target_season: u32) -> bool {
    match extract_season(title) {
        Some(season) if season == target_season => {
            !is_single_episode_release(title) || COMPLETE_SEASON_REGEX.is_match(title)
        }
        _ => false,
    }
}

fn is_single_episode_release(title: &str) -> bool {
    EPISODE_REGEX.is_match(title) || ANIME_EP_MARKER_REGEX.is_match(title)
}

fn has_conflicting_episode(title: &str, target_season: u32, target_episode: u32) -> bool {
    if let (Some(season), Some(episode)) = (extract_season(title), extract_episode(title)) {
        return season == target_season && episode != target_episode;
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fuzzy_match_title() {
        assert_eq!(fuzzy_match_title("The Witcher", "The Witcher"), 1.0);
        assert!(fuzzy_match_title("The Witcher", "The Witcher 3") > 0.7);
        assert!(fuzzy_match_title("Breaking Bad", "Breaking Bad S01") > 0.8);
    }

    #[test]
    fn test_extract_year() {
        assert_eq!(extract_year("Show Title 2020"), Some(2020));
        assert_eq!(extract_year("Show (2021)"), Some(2021));
        assert_eq!(extract_year("Show"), None);
        assert_eq!(extract_all_years("Collection 1977 2018"), vec![1977, 2018]);
    }

    #[test]
    fn test_extract_season() {
        assert_eq!(extract_season("Show S01"), Some(1));
        assert_eq!(extract_season("Show Season 2"), Some(2));
        assert_eq!(extract_season("Show S01E03"), Some(1));
    }

    #[test]
    fn test_extract_episode() {
        assert_eq!(extract_episode("Show S01E03"), Some(3));
        assert_eq!(extract_episode("Show S01E10"), Some(10));
    }

    #[test]
    fn test_year_mismatch_is_penalized() {
        let exact_year = calculate_match_score(
            "Avatar The Last Airbender",
            Some(2005),
            Some(1),
            Some(1),
            "Avatar The Last Airbender S01E01 2005 1080p",
            0,
            0,
        );
        let wrong_year = calculate_match_score(
            "Avatar The Last Airbender",
            Some(2005),
            Some(1),
            Some(1),
            "Avatar The Last Airbender S01E01 2024 1080p",
            0,
            0,
        );

        assert!(exact_year > wrong_year);
    }

    #[test]
    fn test_exact_episode_beats_season_pack() {
        let exact_episode = calculate_match_score(
            "Avatar The Last Airbender",
            Some(2005),
            Some(1),
            Some(1),
            "Avatar The Last Airbender S01E01 1080p",
            0,
            0,
        );
        let season_pack = calculate_match_score(
            "Avatar The Last Airbender",
            Some(2005),
            Some(1),
            Some(1),
            "Avatar The Last Airbender Season 1 Complete 1080p",
            0,
            0,
        );

        assert!(exact_episode > season_pack);
    }

    #[test]
    fn test_required_title_tokens_rejects_missing_numeric_token() {
        assert!(has_required_title_tokens(
            "Jujutsu Kaisen 0",
            "Jujutsu Kaisen 0 Movie 2021 1080p BluRay"
        ));
        assert!(!has_required_title_tokens(
            "Jujutsu Kaisen 0",
            "Jujutsu Kaisen Execution 2025 1080p TS"
        ));
    }

    #[test]
    fn test_required_title_tokens_accepts_partial_coverage_for_longer_titles() {
        assert!(has_required_title_tokens(
            "Avatar The Last Airbender",
            "Avatar Airbender 2005 1080p"
        ));
    }
}
