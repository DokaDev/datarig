#!/usr/bin/env bash
# Loads dev/init/*.sql into the PostgreSQL service container of a CI job (the port of the job's
# DATARIG_TEST_PG_URL on 127.0.0.1), as the dev compose file's container does on its first start.
set -euo pipefail
if ! command -v psql >/dev/null; then
  sudo apt-get update
  sudo apt-get install -y --no-install-recommends postgresql-client
fi
export PGPASSWORD=datarig
url=${DATARIG_TEST_PG_URL:?set DATARIG_TEST_PG_URL}
port=$(sed -E 's|^[^@]*@[^:/]*:([0-9]+)/.*$|\1|' <<< "$url")
for f in dev/init/*.sql; do
  echo "loading $f"
  psql -h 127.0.0.1 -p "$port" -U datarig -d datarig -v ON_ERROR_STOP=1 -q -f "$f"
done
