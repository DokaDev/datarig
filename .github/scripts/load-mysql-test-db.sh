#!/usr/bin/env bash
# Loads dev/init-mysql/*.sql into the MySQL service container of a CI job (the first argument),
# as the dev compose file's container does on its first start, with the container's own client.
set -euo pipefail
container=$1
for f in dev/init-mysql/*.sql; do
  echo "loading $f"
  docker exec -i "$container" mysql -uroot -pdatarig-root --default-character-set=utf8mb4 < "$f"
done
