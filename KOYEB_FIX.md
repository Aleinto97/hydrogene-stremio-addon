# Hydrogene Stremio Addon - Koyeb Deployment Fix

## Problem
Koyeb's Dockerfile builder fails because it can't start the Docker daemon (containerd timeout) on the free tier.

## Solution 1: Use Buildpack (Recommended - Easiest)

1. Go to Koyeb Dashboard → Select your service
2. Click **Settings** tab
3. Under "Deployment definition" → **Builder type**
4. Change from **Dockerfile** to **Buildpack**
5. Click **Redeploy**

Koyeb will auto-detect your Rust project and build it with `cargo build --release`.

## Solution 2: Use Pre-built Docker Image

If you want to keep using Docker, build locally and push to GitHub Container Registry:

```bash
# Build locally
docker build -t ghcr.io/aleinto97/hydrogene:latest .

# Login to GHCR
echo $GITHUB_TOKEN | docker login ghcr.io -u Aleinto97 --password-stdin

# Push
docker push ghcr.io/aleinto97/hydrogene:latest
```

Then in Koyeb Settings, change "Builder type" to **Image** and use:
- Image: `ghcr.io/aleinto97/hydrogene:latest`

## Solution 3: Keep Using Dockerfile with koyeb.yaml

I've added `koyeb.yaml` to the repo. Push this file and Koyeb will use BuildKit which is more reliable:

```yaml
builder:
  type: docker
  buildkit: true
```

## Required Environment Variables

Make sure these are set in Koyeb Dashboard → Settings → Environment variables:

| Variable | Value | Secret |
|----------|-------|--------|
| PORT | 8080 | No |
| DATABASE_URL | postgresql://... | Yes |
| RD_API_KEY | your_key | Yes |
| RUTRACKER_COOKIE | your_cookie | Yes (optional) |

## Troubleshooting Build Failures

If build still fails:
1. **Memory**: Rust builds need >1GB RAM. Upgrade from Free to Starter tier ($5)
2. **Rust version**: The Dockerfile uses Rust 1.88 - Koyeb buildpack uses stable Rust
3. **Build time**: First build takes 5-10 minutes. Be patient.

## Quick Fix Checklist

- [ ] Change builder type to Buildpack in Koyeb Settings
- [ ] Click Redeploy
- [ ] Verify DATABASE_URL is set
- [ ] Wait for build (5-10 min)
