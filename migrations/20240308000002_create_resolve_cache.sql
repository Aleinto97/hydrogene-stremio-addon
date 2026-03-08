-- Create resolve cache table
CREATE TABLE IF NOT EXISTS resolve_cache (
    id SERIAL PRIMARY KEY,
    info_hash TEXT NOT NULL UNIQUE,
    video_url TEXT NOT NULL,
    created_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP
);

-- Create index for faster lookups
CREATE INDEX IF NOT EXISTS idx_resolve_cache_info_hash ON resolve_cache(info_hash);
CREATE INDEX IF NOT EXISTS idx_resolve_cache_created_at ON resolve_cache(created_at);

-- Create cleanup function for expired cache entries
CREATE OR REPLACE FUNCTION cleanup_expired_cache()
RETURNS void AS $$
BEGIN
    DELETE FROM torrent_cache 
    WHERE created_at < NOW() - INTERVAL '24 hours';
    
    DELETE FROM resolve_cache 
    WHERE created_at < NOW() - INTERVAL '24 hours';
END;
$$ language 'plpgsql';