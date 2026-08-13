#!/usr/bin/env bash
# Provisions a CockroachDB Cloud cluster for Arnheid via the ccloud CLI, then
# enables the distributed vector index feature and creates the `arnheid`
# database. Prints what to put in .env.
#
# Prereqs: the ccloud CLI —
#   brew install cockroachdb/tap/ccloud
# (or the Linux/ARM binaries — see
#  https://www.cockroachlabs.com/docs/cockroachcloud/ccloud-get-started)
#
# Usage: ./scripts/provision_cockroachdb.sh
set -euo pipefail

if ! command -v ccloud >/dev/null 2>&1; then
  echo "ccloud CLI not found — install it first:" >&2
  echo "  brew install cockroachdb/tap/ccloud" >&2
  exit 1
fi

echo "Logging in to CockroachDB Cloud (opens a browser)…"
ccloud auth login

echo
echo "Creating a cluster — pick the free Serverless tier when prompted."
ccloud quickstart

cat <<'EOF'

quickstart printed a connection string above — copy it into .env as
DATABASE_URL (append `?sslmode=verify-full` if it isn't already there).

One-time setup on the new cluster — enables the distributed vector index
feature (off by default on self-managed CockroachDB; Cloud Serverless
clusters generally ship with it on, but this is harmless either way) and
creates the database Arnheid expects. Needs the `cockroach` CLI (or paste the
SQL into the Cloud Console's SQL shell instead):

  cockroach sql --url "$DATABASE_URL" -e "
    SET CLUSTER SETTING feature.vector_index.enabled = true;
    CREATE DATABASE IF NOT EXISTS arnheid;
  "

`cargo run` handles the rest (migrations + the dimensioned vector columns
and index) on first boot — see db::ensure_vector_schema in src/db/mod.rs.
EOF
