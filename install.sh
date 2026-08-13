#!/usr/bin/env bash
#
# Arnheid VM installer — Ubuntu 24.04 (Hetzner CX22 or larger).
#
# Installs: Rust toolchain, Ollama + local chat model, builds the release
# binary (embeddings are hosted — set HF_API_KEY in .env),
# and registers a systemd service. The database is CockroachDB, provisioned
# separately (CockroachDB Cloud via `ccloud quickstart`, or self-hosted — see
# scripts/provision_cockroachdb.sh) — this script does not install or manage
# it, only points the service at whatever DATABASE_URL you give it.
#
# Idempotent: safe to re-run. Run as root (or with sudo):
#   DATABASE_URL='postgresql://...cockroachlabs.cloud:26257/arnheid?sslmode=verify-full' \
#     sudo -E bash install.sh
#
# Configure behaviour with env vars before running, e.g.:
#   HF_API_KEY=hf_... EMBED_MODEL=BAAI/bge-base-en-v1.5 EMBED_DIM=768 \
#   INSTALL_CHAT_MODEL=0 sudo -E bash install.sh
#
set -euo pipefail

# ── Tunables ────────────────────────────────────────────────────────────────
APP_USER="${APP_USER:-arnheid}"
APP_DIR="${APP_DIR:-/opt/arnheid}"
ENV_FILE="${APP_DIR}/.env"
EMBED_MODEL="${EMBED_MODEL:-BAAI/bge-large-en-v1.5}"  # hosted on Hugging Face
EMBED_DIM="${EMBED_DIM:-1024}"
CHAT_MODEL="${CHAT_MODEL:-qwen2.5:3b-instruct}"  # Tier 2 (summaries/excerpts); CPU-sized
INSTALL_CHAT_MODEL="${INSTALL_CHAT_MODEL:-1}"     # 1 = pull local chat model (Tier 2 is local by default)

log() { echo -e "\033[1;36m[arnheid]\033[0m $*"; }

if [[ $EUID -ne 0 ]]; then
  echo "Please run as root (sudo bash install.sh)." >&2
  exit 1
fi

# A DATABASE_URL is required on first install (subsequent re-runs reuse the
# one already in .env if you don't pass one). CockroachDB is provisioned
# separately — see scripts/provision_cockroachdb.sh.
if [[ -z "${DATABASE_URL:-}" && ! -f "${ENV_FILE}" ]]; then
  echo "Set DATABASE_URL to a CockroachDB connection string (see scripts/provision_cockroachdb.sh)." >&2
  exit 1
fi

# ── 1. System packages ──────────────────────────────────────────────────────
log "Installing system packages…"
export DEBIAN_FRONTEND=noninteractive
apt-get update -y
apt-get install -y --no-install-recommends \
  build-essential pkg-config libssl-dev ca-certificates curl git

# ── 3. App user + source ─────────────────────────────────────────────────────
if ! id "${APP_USER}" &>/dev/null; then
  log "Creating service user ${APP_USER}…"
  useradd --system --create-home --shell /usr/sbin/nologin "${APP_USER}"
fi

log "Syncing source into ${APP_DIR}…"
mkdir -p "${APP_DIR}"
# If running from a checkout, copy it in; otherwise assume it's already there.
SRC_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
if [[ "${SRC_DIR}" != "${APP_DIR}" ]]; then
  cp -r "${SRC_DIR}/." "${APP_DIR}/"
fi
chown -R "${APP_USER}:${APP_USER}" "${APP_DIR}"

# ── 4. Rust toolchain (installed for the app user) ───────────────────────────
log "Installing Rust toolchain…"
sudo -u "${APP_USER}" bash -c '
  if ! command -v cargo >/dev/null 2>&1 && [ ! -x "$HOME/.cargo/bin/cargo" ]; then
    curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
  fi
'

# ── 5. Ollama + local models ─────────────────────────────────────────────────
log "Installing Ollama…"
if ! command -v ollama >/dev/null 2>&1; then
  curl -fsSL https://ollama.com/install.sh | sh
fi
# On a constrained box, keep one model resident and one request at a time so a
# 7B model can't get loaded twice and blow up RAM. Our ingestion queue is
# sequential anyway, so parallelism buys nothing here.
mkdir -p /etc/systemd/system/ollama.service.d
cat >/etc/systemd/system/ollama.service.d/override.conf <<EOF
[Service]
Environment=OLLAMA_NUM_PARALLEL=1
Environment=OLLAMA_MAX_LOADED_MODELS=1
EOF
systemctl daemon-reload
systemctl enable --now ollama
systemctl restart ollama

# GPU note: Ollama uses a GPU automatically if an NVIDIA driver is present.
if command -v nvidia-smi >/dev/null 2>&1 && nvidia-smi >/dev/null 2>&1; then
  log "NVIDIA GPU detected — Ollama will run models on the GPU."
else
  log "No GPU detected — Ollama runs on CPU (expected for this box). Chat models"
  log "  are CPU-sized (${CHAT_MODEL}); embeddings are hosted on Hugging Face."
fi
# Wait for the daemon to accept connections.
for _ in $(seq 1 30); do
  curl -sf http://localhost:11434/api/tags >/dev/null 2>&1 && break
  sleep 1
done

if [[ "${INSTALL_CHAT_MODEL}" == "1" ]]; then
  log "Pulling local chat model: ${CHAT_MODEL} (used by tiers routed to ollama)…"
  ollama pull "${CHAT_MODEL}"
fi

# ── 5b. YouTube tooling (yt-dlp) ─────────────────────────────────────────────
# Standalone Linux build — no Python runtime dependency. Used to fetch video
# transcripts. If this box isn't x86_64, swap the asset for yt-dlp_linux_aarch64.
log "Installing yt-dlp (YouTube transcript fetcher)…"
curl -fsSL https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_linux \
  -o /usr/local/bin/yt-dlp
chmod a+rx /usr/local/bin/yt-dlp
/usr/local/bin/yt-dlp --version >/dev/null 2>&1 \
  && log "yt-dlp $(/usr/local/bin/yt-dlp --version) ready" \
  || log "WARN: yt-dlp install check failed — YouTube links will store a stub until fixed."

# ── 6. .env scaffold ─────────────────────────────────────────────────────────
if [[ ! -f "${ENV_FILE}" ]]; then
  log "Writing ${ENV_FILE} (fill in TELEGRAM_BOT_TOKEN + ANTHROPIC_API_KEY)…"
  cp "${APP_DIR}/.env.example" "${ENV_FILE}"
  sed -i "s#^DATABASE_URL=.*#DATABASE_URL=${DATABASE_URL}#" "${ENV_FILE}"
  sed -i "s#^EMBEDDING_MODEL=.*#EMBEDDING_MODEL=${EMBED_MODEL}#" "${ENV_FILE}"
  if [[ -n "${HF_API_KEY:-}" ]]; then
    sed -i "s#^HF_API_KEY=.*#HF_API_KEY=${HF_API_KEY}#" "${ENV_FILE}"
  fi
  sed -i "s#^EMBEDDING_DIM=.*#EMBEDDING_DIM=${EMBED_DIM}#" "${ENV_FILE}"
  sed -i "s#^OLLAMA_CHAT_MODEL=.*#OLLAMA_CHAT_MODEL=${CHAT_MODEL}#" "${ENV_FILE}"
  chown "${APP_USER}:${APP_USER}" "${ENV_FILE}"
  chmod 600 "${ENV_FILE}"
else
  log ".env already exists — leaving it untouched."
fi

# ── 7. Build release binary ──────────────────────────────────────────────────
log "Building release binary (this takes a few minutes)…"
sudo -u "${APP_USER}" bash -c "cd '${APP_DIR}' && \$HOME/.cargo/bin/cargo build --release"

# ── 8. systemd service ───────────────────────────────────────────────────────
log "Installing systemd service…"
cat >/etc/systemd/system/arnheid.service <<EOF
[Unit]
Description=Arnheid Bot
After=network.target ollama.service
Wants=ollama.service

[Service]
Type=simple
User=${APP_USER}
WorkingDirectory=${APP_DIR}
EnvironmentFile=${APP_DIR}/.env
ExecStart=${APP_DIR}/target/release/arnheid
Restart=always
RestartSec=5
StandardOutput=journal
StandardError=journal

[Install]
WantedBy=multi-user.target
EOF

systemctl daemon-reload
systemctl enable arnheid

cat <<DONE

────────────────────────────────────────────────────────────
 Arnheid installed.

 1. Edit ${ENV_FILE} and set:
      TELEGRAM_BOT_TOKEN=...
      ANTHROPIC_API_KEY=...        (used for graph + /ask; Tier 2 runs locally)
      HF_API_KEY=hf_...            (embeddings — required; "Inference Providers" scope)
    DB URL, embedding + chat model settings are already filled in.

 2. Start it:
      systemctl start arnheid
      journalctl -u arnheid -f

 Database:                     CockroachDB — $([[ -n "${DATABASE_URL:-}" ]] && echo "${DATABASE_URL}" || echo "(kept from existing .env)")
 Embedding model:              ${EMBED_MODEL} (dim ${EMBED_DIM}, hosted on Hugging Face)
 Local chat model pulled:      $([[ "${INSTALL_CHAT_MODEL}" == "1" ]] && echo "${CHAT_MODEL}" || echo "no (set INSTALL_CHAT_MODEL=1 to enable)")
────────────────────────────────────────────────────────────
DONE
