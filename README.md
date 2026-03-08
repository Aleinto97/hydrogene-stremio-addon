# Hydrogen - Stremio Torrent Addon

Un addon Stremio ad alte prestazioni scritto in Rust che aggrega torrent da multiple fonti e li streamma tramite Real-Debrid.

## 🏗️ Architettura

### Stack Tecnologico
- **Core & Web Server**: Rust + Axum (ultraleggero, gestione rotte a velocità record)
- **HTTP Client**: reqwest-impersonate (bypass Cloudflare fingendo di essere Chrome)
- **Database**: Supabase (PostgreSQL) con sqlx per query asincrone
- **Debrid**: Real-Debrid API per stream ad alta velocità
- **Hosting**: Koyeb (Free Tier, 512MB RAM) con Docker Multi-Stage

### Struttura Moduli
```
src/
├── main.rs           # Entry point, server Axum, rotte
├── db.rs             # Pool connessioni Supabase, cache query
├── debrid.rs         # Integrazione Real-Debrid API
└── scrapers/
    ├── mod.rs        # Orchestratore scraper paralleli
    ├── nyaa.rs       # Nyaa + Sukebei (anime)
    ├── tpb.rs        # The Pirate Bay
    ├── rutor.rs      # Rutor
    ├── rutracker.rs  # RuTracker (richiede cookie)
    └── watchsomuch.rs # WatchSoMuch
```

## 🚀 Flusso di Lavoro

### 1. Richiesta Stremio
```
GET /stream/movie/tt1375666.json
```

### 2. Controllo Cache (Supabase)
- Se cache valida: skip scraping → punto 4
- Se cache miss: procedi punto 3

### 3. Scraping Parallelo
- Tokio lancia tutti gli scraper contemporaneamente
- Risultati de-duplicati per info_hash
- Ordinati per seeders (migliori primi)
- Salvataggio cache Supabase

### 4. Presentazione Stremio
- Formattazione risultati JSON compatibile Stremio
- Metadata: dimensione, seeders, fonte

### 5. Risoluzione Real-Debrid
```
GET /resolve/{hash}
```
- `/torrents/addMagnet` → aggiunge hash
- `/torrents/info/{id}` → attende metadati
- `/torrents/selectFiles/{id}` → seleziona video
- `/torrents/selectFiles/{id}` → attende download
- `/unrestrict/link` → ottiene URL MP4 diretto

### 6. Play
- Redirect 302 al link video RD
- Film parte istantaneamente

## 🛠️ Setup Locale

### 1. Prerequisiti
- Rust 1.75+
- PostgreSQL (o account Supabase)
- Real-Debrid API Key

### 2. Installazione
```bash
# Clona repository
git clone <repo-url>
cd hydrogene

# Copia configurazione
cp .env.example .env

# Edita .env con i tuoi valori:
# - DATABASE_URL (Supabase PostgreSQL)
# - RD_API_KEY (da real-debrid.com/apitoken)
# - RUTRACKER_COOKIE (opzionale)

# Installa dipendenze e compila
cargo build --release

# Esegui migrazioni database
# (sqlx le esegue automaticamente all'avvio)

# Avvia server
cargo run
```

### 3. Test
```bash
# Verifica server in esecuzione
curl http://localhost:8080/

# Test manifest
curl http://localhost:8080/manifest.json

# Test stream (sostituisci con un ID IMDB reale)
curl http://localhost:8080/stream/movie/tt1375666.json
```

## 🐳 Deploy su Koyeb

### 1. Preparazione
```bash
# Assicurati che il progetto compili localmente
cargo build --release

# Build Docker image
docker build -t stremio-addon .
```

### 2. Configurazione Koyeb

1. Crea nuovo servizio su [Koyeb Console](https://app.koyeb.com)
2. Scegli "Deploy from GitHub repository"
3. Seleziona il tuo repo
4. Builder: `Docker`
5. Dockerfile path: `./Dockerfile`
6. Port: `8080`

### 3. Variabili d'Ambiente
Configura in Koyeb Dashboard → Settings → Environment Variables:

```
PORT=8080
DATABASE_URL=postgresql://postgres:[password]@db.[project].supabase.co:5432/postgres?sslmode=require
RD_API_KEY=il_tuo_api_key_reale
CACHE_TTL_HOURS=24
MAX_CONCURRENT_SCRAPERS=5
```

### 4. Domini
Koyeb assegna automaticamente un dominio:
`https://tuoservizio.koyeb.app`

Aggiungi a Stremio come:
`https://tuoservizio.koyeb.app/manifest.json`

## 📊 Performance

- **RAM utilizzata**: ~30MB (ben al di sotto del limite 512MB Koyeb)
- **Tempo risposta**: <2 secondi con cache, <5 secondi con scraping
- **Connessioni DB**: 10 max, 2 min
- **Scraper concorrenti**: 5 (configurabile)

## 🔧 Configurazione Avanzata

### RuTracker
Per usare RuTracker:
1. Registrati su rutracker.org
2. Effettua login
3. Apri DevTools → Application → Cookies
4. Copia il valore del cookie `bb_session`
5. Imposta `RUTRACKER_COOKIE=il_tuo_cookie` in .env/Koyeb

### Cache TTL
Modifica `CACHE_TTL_HOURS` per cambiare durata cache:
- Valori bassi (1-6h): dati più freschi, più scraping
- Valori alti (24-72h): meno scraping, dati più vecchi

## 🐛 Debug

```bash
# Log verboso
RUST_LOG=debug cargo run

# Solo errori
RUST_LOG=error cargo run

# Struttura log
RUST_LOG=info,stremio_addon=debug cargo run
```

## 📝 API Endpoints

| Endpoint | Metodo | Descrizione |
|----------|--------|-------------|
| `/` | GET | Health check |
| `/manifest.json` | GET | Manifest addon Stremio |
| `/stream/:type/:id.json` | GET | Lista stream per contenuto |
| `/resolve/:hash` | GET | Risolve hash → URL video |

## 🤝 Contribuire

Contributi welcome! Fork, branch, PR.

## 📄 License

MIT License - vedi LICENSE

---

**Made with** 🦀 **in Italy**