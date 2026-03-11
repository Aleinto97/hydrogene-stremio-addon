pub mod cache;
pub mod debrid;
pub mod hydra;
pub mod matching;
pub mod meta_index;
pub mod metadata;
pub mod scrapers;
pub mod stremio_format;
pub mod utils;

pub use cache::MetadataCache;
pub use debrid::ResolveResult;
pub use hydra::HydraId;
pub use matching::calculate_match_score;
pub use meta_index::{CatalogMeta, MetaItem, MetadataIndex};
pub use stremio_format::{StremioFormatter, StremioStream, TorrentInfo};
