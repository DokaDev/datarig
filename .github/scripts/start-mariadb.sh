#!/usr/bin/env bash
# Starts the MariaDB server of the `integration (MariaDB)` CI job and loads dev/init-mysql/*.sql
# into it, as dev/docker-compose.yml's `mariadb` service does. A service container cannot be
# given server options, and performance_schema (off by default on MariaDB) can only be turned on
# at start: the tests look at the driver's sessions from the side through it (connection
# attributes, statement history, metadata locks).
#   start-mariadb.sh <image> <host port>
set -euo pipefail
image=$1
port=$2
name=zz_datarig_mariadb
docker run -d --name "$name" -p "127.0.0.1:$port:3306" \
  -e MARIADB_USER=datarig -e MARIADB_PASSWORD=datarig -e MARIADB_DATABASE=datarig \
  -e MARIADB_ROOT_PASSWORD=datarig-root "$image" \
  --performance-schema=ON \
  --performance-schema-consumer-events-statements-current=ON \
  --performance-schema-consumer-events-statements-history=ON \
  --performance-schema-instrument=wait/lock/metadata/sql/mdl=ON
# Over TCP: the image's first, temporary server (it makes the users) listens on its socket only.
for _ in $(seq 1 90); do
  if docker exec "$name" mariadb -h127.0.0.1 -P3306 --protocol=TCP -uroot -pdatarig-root -e 'SELECT 1' >/dev/null 2>&1; then
    break
  fi
  sleep 2
done
docker exec "$name" mariadb -h127.0.0.1 -P3306 --protocol=TCP -uroot -pdatarig-root -N -e 'SELECT VERSION()'
for f in dev/init-mysql/*.sql; do
  echo "loading $f"
  docker exec -i "$name" mariadb -uroot -pdatarig-root --default-character-set=utf8mb4 < "$f"
done
