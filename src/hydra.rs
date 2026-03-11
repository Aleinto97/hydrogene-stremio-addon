use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HydraKind {
    Movie,
    Series,
    Anime,
}

impl HydraKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Movie => "movie",
            Self::Series => "series",
            Self::Anime => "anime",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HydraSource {
    Tt,
    Tmdb,
    Anidb,
}

impl HydraSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Tt => "tt",
            Self::Tmdb => "tmdb",
            Self::Anidb => "anidb",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HydraId {
    pub kind: HydraKind,
    pub source: HydraSource,
    pub primary_id: String,
    pub season: Option<u32>,
    pub episode: Option<u32>,
}

impl HydraId {
    pub fn new(kind: HydraKind, source: HydraSource, primary_id: impl Into<String>) -> Self {
        Self {
            kind,
            source,
            primary_id: primary_id.into(),
            season: None,
            episode: None,
        }
    }

    pub fn with_episode(mut self, season: u32, episode: u32) -> Self {
        self.season = Some(season);
        self.episode = Some(episode);
        self
    }

    pub fn base_key(&self) -> String {
        format!(
            "hydra:{}:{}:{}",
            self.kind.as_str(),
            self.source.as_str(),
            self.primary_id
        )
    }

    pub fn parse(id: &str) -> Option<Self> {
        let parts: Vec<&str> = id.split(':').collect();
        if parts.len() < 4 || parts.first().copied()? != "hydra" {
            return None;
        }

        let kind = match parts.get(1).copied()? {
            "movie" => HydraKind::Movie,
            "series" => HydraKind::Series,
            "anime" => HydraKind::Anime,
            _ => return None,
        };

        let source = match parts.get(2).copied()? {
            "tt" => HydraSource::Tt,
            "tmdb" => HydraSource::Tmdb,
            "anidb" => HydraSource::Anidb,
            _ => return None,
        };

        let primary_id = parts.get(3)?.to_string();
        let season = parts.get(4).and_then(|value| value.parse::<u32>().ok());
        let episode = parts.get(5).and_then(|value| value.parse::<u32>().ok());

        Some(Self {
            kind,
            source,
            primary_id,
            season,
            episode,
        })
    }
}

impl Display for HydraId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "hydra:{}:{}:{}",
            self.kind.as_str(),
            self.source.as_str(),
            self.primary_id
        )?;

        if let (Some(season), Some(episode)) = (self.season, self.episode) {
            write!(f, ":{}:{}", season, episode)?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_base_hydra_id() {
        let hydra = HydraId::parse("hydra:movie:tt:tt1375666").unwrap();
        assert_eq!(hydra.kind, HydraKind::Movie);
        assert_eq!(hydra.source, HydraSource::Tt);
        assert_eq!(hydra.primary_id, "tt1375666");
        assert_eq!(hydra.season, None);
        assert_eq!(hydra.episode, None);
    }

    #[test]
    fn parse_episode_hydra_id() {
        let hydra = HydraId::parse("hydra:anime:anidb:16498:1:1").unwrap();
        assert_eq!(hydra.kind, HydraKind::Anime);
        assert_eq!(hydra.source, HydraSource::Anidb);
        assert_eq!(hydra.primary_id, "16498");
        assert_eq!(hydra.season, Some(1));
        assert_eq!(hydra.episode, Some(1));
        assert_eq!(hydra.to_string(), "hydra:anime:anidb:16498:1:1");
    }
}
