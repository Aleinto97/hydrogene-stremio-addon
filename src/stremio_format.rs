use crate::scrapers::ScrapedTorrent;
use std::collections::HashMap;

/// Struttura per le informazioni estratte da un titolo torrent
#[derive(Debug, Clone, Default)]
pub struct TorrentInfo {
    pub provider: String,
    pub filename: String,
    pub resolution: String,     // 4K, 1080p, 720p, ecc.
    pub source_type: String,    // REMUX, WEB-DL, BDRip, ecc.
    pub video_codec: String,    // HEVC, AVC, AV1, ecc.
    pub audio_codec: String,    // TrueHD, EAC3, AAC, ecc.
    pub audio_channels: String, // 7.1, 5.1, 2.0, ecc.
    pub size_gb: f64,
    pub seeders: i32,
    pub leechers: i32,
    pub languages: Vec<String>, // ITA, ENG, MULTI, ecc.
    pub hdr_format: String,     // HDR10, Dolby Vision, ecc.
    pub info_hash: String,
}

impl TorrentInfo {
    /// Crea TorrentInfo da un ScrapedTorrent
    pub fn from_scraped_torrent(torrent: &ScrapedTorrent) -> Self {
        StremioFormatter::parse_title(
            &torrent.source,
            &torrent.title,
            torrent.size_gb,
            torrent.seeders,
            torrent.leechers,
            &torrent.info_hash,
        )
    }
}

/// Formatter per stream Stremio secondo layout visivo ottimale
pub struct StremioFormatter;

impl StremioFormatter {
    /// Formatta il campo name: [Provider]\n[Emoji] [Risoluzione] [Sorgente]
    pub fn format_name(info: &TorrentInfo) -> String {
        let resolution_emoji = Self::get_resolution_emoji(&info.resolution);
        let source_emoji = Self::get_source_emoji(&info.source_type);

        format!(
            "[{}]\n{} {} {}{}",
            info.provider,
            resolution_emoji,
            info.resolution,
            source_emoji,
            if info.source_type.is_empty() {
                "".to_string()
            } else {
                format!(" {}", info.source_type)
            }
        )
    }

    /// Formatta il campo title: Nome file\n📦 Peso | 🌍 Lingua\n🎥 Video | 🔊 Audio
    pub fn format_title(info: &TorrentInfo) -> String {
        let size_str = Self::format_size(info.size_gb);
        let lang_str = Self::format_languages(&info.languages);
        let video_str = Self::format_video_info(info);
        let audio_str = Self::format_audio_info(info);

        format!(
            "{}\n📦 {}  |  {}\n🎥 {}  |  🔊 {}",
            info.filename, size_str, lang_str, video_str, audio_str
        )
    }

    /// Estrae tutte le informazioni da un titolo torrent
    pub fn parse_title(
        provider: &str,
        title: &str,
        size_gb: f64,
        seeders: i32,
        leechers: i32,
        info_hash: &str,
    ) -> TorrentInfo {
        let title_upper = title.to_uppercase();

        TorrentInfo {
            provider: provider.to_string(),
            filename: title.to_string(),
            resolution: Self::extract_resolution(&title_upper),
            source_type: Self::extract_source(&title_upper),
            video_codec: Self::extract_video_codec(&title_upper),
            audio_codec: Self::extract_audio_codec(&title_upper),
            audio_channels: Self::extract_audio_channels(&title_upper),
            size_gb,
            seeders,
            leechers,
            languages: Self::extract_languages(&title_upper),
            hdr_format: Self::extract_hdr_format(&title_upper),
            info_hash: info_hash.to_string(),
        }
    }

    // ===== Estrazione informazioni =====

    fn extract_resolution(title: &str) -> String {
        if title.contains("2160P") || title.contains("4K") || title.contains("UHD") {
            "2160p".to_string()
        } else if title.contains("1080P") {
            "1080p".to_string()
        } else if title.contains("720P") {
            "720p".to_string()
        } else if title.contains("480P") {
            "480p".to_string()
        } else {
            "SD".to_string()
        }
    }

    fn extract_source(title: &str) -> String {
        if title.contains("REMUX") {
            "REMUX".to_string()
        } else if title.contains("WEB-DL") || title.contains("WEBDL") {
            "WEB-DL".to_string()
        } else if title.contains("WEBRIP") || title.contains("WEB-RIP") {
            "WEBRip".to_string()
        } else if title.contains("BLURAY") || title.contains("BLU-RAY") {
            "BluRay".to_string()
        } else if title.contains("BDRIP") || title.contains("BD-RIP") {
            "BDRip".to_string()
        } else if title.contains("HDTV") {
            "HDTV".to_string()
        } else if title.contains("HDRIP") {
            "HDRip".to_string()
        } else if title.contains("DVDRIP") {
            "DVDRip".to_string()
        } else if title.contains("HDCAM") || title.contains("HD-CAM") {
            "HDCAM".to_string()
        } else if title.contains("CAM") && !title.contains("HDCAM") {
            "CAM".to_string()
        } else if title.contains("TS") || title.contains("TELESYNC") {
            "TS".to_string()
        } else {
            String::new()
        }
    }

    fn extract_video_codec(title: &str) -> String {
        if title.contains("X265")
            || title.contains("HEVC")
            || title.contains("H.265")
            || title.contains("H265")
        {
            "HEVC (x265)".to_string()
        } else if title.contains("X264")
            || title.contains("H.264")
            || title.contains("H264")
            || title.contains("AVC")
        {
            "AVC (x264)".to_string()
        } else if title.contains("AV1") {
            "AV1".to_string()
        } else if title.contains("VP9") {
            "VP9".to_string()
        } else {
            String::new()
        }
    }

    fn extract_audio_codec(title: &str) -> String {
        if title.contains("TRUEHD") || title.contains("TRUE-HD") {
            "TrueHD".to_string()
        } else if title.contains("ATMOS") {
            "Atmos".to_string()
        } else if title.contains("DTS-HD") || title.contains("DTSHD") {
            "DTS-HD MA".to_string()
        } else if title.contains("DTS") {
            "DTS".to_string()
        } else if title.contains("DDP5.1")
            || title.contains("EAC3")
            || title.contains("E-AC3")
            || title.contains("DD+")
        {
            "EAC3 (DD+)".to_string()
        } else if title.contains("DD5.1")
            || title.contains("AC3")
            || title.contains("DOLBY DIGITAL")
        {
            "AC3 (DD)".to_string()
        } else if title.contains("AAC") {
            "AAC".to_string()
        } else if title.contains("MP3") {
            "MP3".to_string()
        } else if title.contains("FLAC") {
            "FLAC".to_string()
        } else {
            String::new()
        }
    }

    fn extract_audio_channels(title: &str) -> String {
        if title.contains("7.1") || title.contains("7.1CH") {
            "7.1".to_string()
        } else if title.contains("5.1") || title.contains("5.1CH") || title.contains("5 1") {
            "5.1".to_string()
        } else if title.contains("2.0")
            || title.contains("2.0CH")
            || title.contains("2CH")
            || title.contains("STEREO")
        {
            "2.0".to_string()
        } else if title.contains("1.0") || title.contains("MONO") {
            "1.0".to_string()
        } else {
            String::new()
        }
    }

    fn extract_hdr_format(title: &str) -> String {
        if title.contains("DV") || title.contains("DOBY VISION") || title.contains("DOVI") {
            "Dolby Vision".to_string()
        } else if title.contains("HDR10+") || title.contains("HDR10PLUS") {
            "HDR10+".to_string()
        } else if title.contains("HDR10") {
            "HDR10".to_string()
        } else if title.contains("HDR") {
            "HDR".to_string()
        } else if title.contains("HLG") {
            "HLG".to_string()
        } else {
            String::new()
        }
    }

    fn extract_languages(title: &str) -> Vec<String> {
        let mut langs = Vec::new();

        // Pattern comuni per lingue nei titoli torrent
        let lang_patterns: HashMap<&str, Vec<&str>> = [
            ("ITA", vec!["ITA", "ITALIAN", "ITALIANO", "IT-IT", "IT"]),
            ("ENG", vec!["ENG", "ENGLISH", "EN", "EN-US", "EN-GB"]),
            ("SPA", vec!["SPA", "SPANISH", "ES", "ESPANOL", "CASTELLANO"]),
            ("FRA", vec!["FRA", "FRENCH", "FR", "FRANCAIS", "VF"]),
            ("GER", vec!["GER", "GERMAN", "DE", "DEU", "DEUTSCH", "DL"]),
            ("JPN", vec!["JPN", "JAPANESE", "JA", "JP"]),
            ("KOR", vec!["KOR", "KOREAN", "KO", "KR"]),
            (
                "CHI",
                vec!["CHI", "CHINESE", "ZH", "CN", "MANDARIN", "CANTONESE"],
            ),
            ("RUS", vec!["RUS", "RUSSIAN", "RU", "RUSSKIJ"]),
            ("POL", vec!["POL", "POLISH", "PL", "POLSKI"]),
            ("POR", vec!["POR", "PORTUGUESE", "PT", "PT-BR", "BR"]),
            ("MULTI", vec!["MULTI", "MULTILINGUAL", "DUAL", "DUAL-AUDIO"]),
        ]
        .iter()
        .cloned()
        .collect();

        for (lang_code, patterns) in lang_patterns {
            for pattern in patterns {
                if title.contains(&format!("{}]", pattern))
                    || title.contains(&format!("[{}]", pattern))
                    || title.contains(&format!("_{}", pattern))
                    || title.contains(&format!("{}-", pattern))
                    || title.contains(&format!(" { } ", pattern))
                    || title.contains(&format!("({})", pattern))
                    || title.contains(&format!(".{}", pattern))
                {
                    if !langs.contains(&lang_code.to_string()) {
                        langs.push(lang_code.to_string());
                    }
                    break;
                }
            }
        }

        // Se nessuna lingua trovata, controlla se c'è "MULTI"
        if langs.is_empty() && title.contains("MULTI") {
            langs.push("MULTI".to_string());
        }

        langs
    }

    // ===== Emoji helpers =====

    fn get_resolution_emoji(resolution: &str) -> &'static str {
        match resolution {
            "2160p" | "4K" => "🌟",
            "1080p" => "🎬",
            "720p" => "📺",
            _ => "📼",
        }
    }

    fn get_source_emoji(source: &str) -> &'static str {
        match source {
            "REMUX" | "BluRay" => "💿",
            "WEB-DL" | "WEBRip" => "🌐",
            "BDRip" | "HDRip" => "🎞️",
            "HDTV" => "📡",
            "CAM" | "TS" | "HDCAM" => "📹",
            _ => "🎥",
        }
    }

    fn format_size(size_gb: f64) -> String {
        if size_gb >= 10.0 {
            format!("{:.1} GB", size_gb)
        } else if size_gb >= 1.0 {
            format!("{:.2} GB", size_gb)
        } else if size_gb > 0.0 {
            format!("{:.0} MB", size_gb * 1024.0)
        } else {
            "N/A".to_string()
        }
    }

    fn format_languages(langs: &[String]) -> String {
        if langs.is_empty() {
            return "🌍 MULTI".to_string();
        }

        if langs.contains(&"MULTI".to_string()) || langs.len() > 2 {
            return "🌍 MULTI".to_string();
        }

        let lang_emojis: HashMap<&str, &str> = [
            ("ITA", "🇮🇹"),
            ("ENG", "🇬🇧"),
            ("SPA", "🇪🇸"),
            ("FRA", "🇫🇷"),
            ("GER", "🇩🇪"),
            ("JPN", "🇯🇵"),
            ("KOR", "🇰🇷"),
            ("CHI", "🇨🇳"),
            ("RUS", "🇷🇺"),
            ("POL", "🇵🇱"),
            ("POR", "🇵🇹"),
        ]
        .iter()
        .cloned()
        .collect();

        let formatted: Vec<String> = langs
            .iter()
            .map(|l| {
                if let Some(emoji) = lang_emojis.get(l.as_str()) {
                    format!("{} {}", emoji, l)
                } else {
                    format!("🌍 {}", l)
                }
            })
            .collect();

        formatted.join(" / ")
    }

    fn format_video_info(info: &TorrentInfo) -> String {
        let mut parts = Vec::new();

        if !info.video_codec.is_empty() {
            parts.push(info.video_codec.clone());
        }

        if !info.hdr_format.is_empty() {
            parts.push(info.hdr_format.clone());
        }

        if parts.is_empty() {
            "N/A".to_string()
        } else {
            parts.join(" ")
        }
    }

    fn format_audio_info(info: &TorrentInfo) -> String {
        let mut parts = Vec::new();

        if !info.audio_codec.is_empty() {
            parts.push(info.audio_codec.clone());
        }

        if !info.audio_channels.is_empty() {
            parts.push(format!("{}ch", info.audio_channels));
        }

        if parts.is_empty() {
            "N/A".to_string()
        } else {
            parts.join(" ")
        }
    }
}

/// Genera uno stream Stremio formattato da un TorrentInfo
#[derive(serde::Serialize)]
pub struct StremioStream {
    pub name: String,
    pub title: String,
    #[serde(rename = "infoHash")]
    pub info_hash: Option<String>,
    #[serde(rename = "url")]
    pub url: Option<String>,
    #[serde(rename = "behaviorHints")]
    pub behavior_hints: serde_json::Value,
}

impl StremioStream {
    pub fn from_torrent_info(info: &TorrentInfo, base_url: &str) -> Self {
        Self {
            name: StremioFormatter::format_name(info),
            title: StremioFormatter::format_title(info),
            info_hash: Some(info.info_hash.clone()),
            url: Some(format!("{}/resolve/{}", base_url, info.info_hash)),
            behavior_hints: serde_json::json!({
                "bingeGroup": format!("torrent-{}", info.provider),
                "filename": info.filename
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_name() {
        let info = TorrentInfo {
            provider: "TPB".to_string(),
            resolution: "2160p".to_string(),
            source_type: "REMUX".to_string(),
            ..Default::default()
        };

        let name = StremioFormatter::format_name(&info);
        assert_eq!(name, "[TPB]\n🌟 2160p 💿 REMUX");
    }

    #[test]
    fn test_format_title() {
        let info = TorrentInfo {
            filename: "Inception.2010.2160p.BluRay.Remux.HEVC.TrueHD.7.1.mkv".to_string(),
            size_gb: 65.2,
            languages: vec!["ITA".to_string(), "ENG".to_string()],
            video_codec: "HEVC (x265)".to_string(),
            hdr_format: "HDR10".to_string(),
            audio_codec: "TrueHD".to_string(),
            audio_channels: "7.1".to_string(),
            ..Default::default()
        };

        let title = StremioFormatter::format_title(&info);
        assert!(title.contains("Inception.2010.2160p.BluRay.Remux.HEVC.TrueHD.7.1.mkv"));
        assert!(title.contains("📦 65.2 GB"));
        assert!(title.contains("🇮🇹 ITA / 🇬🇧 ENG"));
        assert!(title.contains("🎥 HEVC (x265) HDR10"));
        assert!(title.contains("🔊 TrueHD 7.1ch"));
    }

    #[test]
    fn test_parse_title() {
        let info = StremioFormatter::parse_title(
            "RARBG",
            "Inception.2010.1080p.AMZN.WEB-DL.DDP5.1.H.264.mkv",
            8.4,
            150,
            20,
            "abc123",
        );

        assert_eq!(info.provider, "RARBG");
        assert_eq!(info.resolution, "1080p");
        assert_eq!(info.source_type, "WEB-DL");
        assert_eq!(info.video_codec, "AVC (x264)");
        assert_eq!(info.audio_codec, "EAC3 (DD+)");
        assert_eq!(info.audio_channels, "5.1");
    }
}
