// Esempio di output JSON generato dal nuovo formatter

use hydrogene::stremio_format::{StremioFormatter, StremioStream, TorrentInfo};

fn main() {
    // Esempio 1: 4K REMUX da TPB
    let torrent1 = TorrentInfo {
        provider: "TPB".to_string(),
        filename: "Inception.2010.2160p.BluRay.Remux.HEVC.TrueHD.7.1.mkv".to_string(),
        resolution: "2160p".to_string(),
        source_type: "REMUX".to_string(),
        video_codec: "HEVC (x265)".to_string(),
        audio_codec: "TrueHD".to_string(),
        audio_channels: "7.1".to_string(),
        size_gb: 65.2,
        seeders: 150,
        leechers: 20,
        languages: vec!["ITA".to_string(), "ENG".to_string()],
        hdr_format: "HDR10".to_string(),
        info_hash: "a1b2c3d4e5f6g7h8i9j0".to_string(),
    };

    println!("=== ESEMPIO 1: 4K REMUX ===");
    println!("name: {}", StremioFormatter::format_name(&torrent1));
    println!();
    println!("title: {}", StremioFormatter::format_title(&torrent1));
    println!();

    // Esempio 2: 1080p WEB-DL da RARBG
    let torrent2 = TorrentInfo {
        provider: "RARBG".to_string(),
        filename: "Inception.2010.1080p.AMZN.WEB-DL.DDP5.1.H.264.mkv".to_string(),
        resolution: "1080p".to_string(),
        source_type: "WEB-DL".to_string(),
        video_codec: "AVC (x264)".to_string(),
        audio_codec: "EAC3 (DD+)".to_string(),
        audio_channels: "5.1".to_string(),
        size_gb: 8.4,
        seeders: 230,
        leechers: 15,
        languages: vec!["MULTI".to_string()],
        hdr_format: "".to_string(),
        info_hash: "z0y9x8w7v6u5t4s3r2q1".to_string(),
    };

    println!("=== ESEMPIO 2: 1080p WEB-DL ===");
    println!("name: {}", StremioFormatter::format_name(&torrent2));
    println!();
    println!("title: {}", StremioFormatter::format_title(&torrent2));
    println!();

    // Esempio 3: 4K WEB-DL con Dolby Vision
    let torrent3 = TorrentInfo {
        provider: "1337x".to_string(),
        filename: "Dune.Part.Two.2024.2160p.MA.WEB-DL.DDP5.1.Atmos.DV.HDR.H.265.mkv".to_string(),
        resolution: "2160p".to_string(),
        source_type: "WEB-DL".to_string(),
        video_codec: "HEVC (x265)".to_string(),
        audio_codec: "EAC3 (DD+)".to_string(),
        audio_channels: "5.1".to_string(),
        size_gb: 24.5,
        seeders: 89,
        leechers: 8,
        languages: vec!["ENG".to_string()],
        hdr_format: "Dolby Vision".to_string(),
        info_hash: "c3d4e5f6g7h8i9j0k1l2".to_string(),
    };

    println!("=== ESEMPIO 3: 4K Dolby Vision ===");
    println!("name: {}", StremioFormatter::format_name(&torrent3));
    println!();
    println!("title: {}", StremioFormatter::format_title(&torrent3));
    println!();

    // JSON finale
    let base_url = "http://localhost:8080";
    let stream = StremioStream::from_torrent_info(&torrent1, base_url);

    println!("=== JSON OUTPUT ===");
    println!("{}", serde_json::to_string_pretty(&stream).unwrap());
}
