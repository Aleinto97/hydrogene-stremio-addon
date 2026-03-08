-- Create torrent cache table
CREATE TABLE IF NOT EXISTS torrent_cache (
    id SERIAL PRIMARY KEY,
    imdb_id TEXT NOT NULL,
    title TEXT NOT NULL,
    info_hash TEXT NOT NULL,
    magnet_link TEXT NOT NULL,
    size_bytes BIGINT NOT NULL DEFAULT 0,
    seeders INTEGER NOT NULL DEFAULT 0,
    leechers INTEGER NOT NULL DEFAULT 0,
    source TEXT NOT NULL,
    category TEXT NOT NULL DEFAULT 'Unknown',
    created_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(imdb_id, info_hash, source)
);

-- Create index for faster lookups
CREATE INDEX IF NOT EXISTS idx_torrent_cache_imdb_id ON torrent_cache(imdb_id);
CREATE INDEX IF NOT EXISTS idx_torrent_cache_created_at ON torrent_cache(created_at);
CREATE INDEX IF NOT EXISTS idx_torrent_cache_info_hash ON torrent_cache(info_hash);

-- Create function to update updated_at timestamp
CREATE OR REPLACE FUNCTION update_updated_at_column()
RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at = CURRENT_TIMESTAMP;
    RETURN NEW;
END;
$$ language 'plpgsql';

-- Create trigger to auto-update updated_at
DROP TRIGGER IF EXISTS update_torrent_cache_updated_at ON torrent_cache;
CREATE TRIGGER update_torrent_cache_updated_at
    BEFORE UPDATE ON torrent_cache
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_at_column();