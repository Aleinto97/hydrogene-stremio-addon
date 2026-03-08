# Build locally and push to GitHub Container Registry
docker build -t ghcr.io/aleinto97/hydrogene:latest .

# Login (create PAT at https://github.com/settings/tokens with 'write:packages')
echo YOUR_GITHUB_TOKEN | docker login ghcr.io -u Aleinto97 --password-stdin

# Push
docker push ghcr.io/aleinto97/hydrogene:latest
