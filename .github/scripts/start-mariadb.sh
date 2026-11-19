#!/usr/bin/env bash
# Starts the MariaDB server of the `integration (MariaDB)` CI job and loads dev/init-mysql/*.sql
# into it, as dev/docker-compose.yml's `mariadb` service does. A service container cannot be
# given server options, and performance_schema (off by default on MariaDB) can only be turned on
# at start: the tests look at the driver's sessions from the side through it (connection
# attributes, statement history, metadata locks).
# Given `ps=off`, the server starts without those options, so with performance_schema off as
# MariaDB ships: the cancel tests that need such a server run against it.
#   start-mariadb.sh <image> <host port> [<container name> [ps=off]]
set -euo pipefail
image=$1
port=$2
name=${3:-zz_datarig_mariadb}
ps=${4:-ps=on}
case "$ps" in
  ps=on)
    options=(
      --performance-schema=ON
      --performance-schema-consumer-events-statements-current=ON
      --performance-schema-consumer-events-statements-history=ON
      --performance-schema-instrument=wait/lock/metadata/sql/mdl=ON
    )
    ;;
  ps=off) options=() ;;
  *)
    echo "unknown fourth argument: $ps (ps=off or none)" >&2
    exit 2
    ;;
esac
docker run -d --name "$name" -p "127.0.0.1:$port:3306" \
  -e MARIADB_USER=datarig -e MARIADB_PASSWORD=datarig -e MARIADB_DATABASE=datarig \
  -e MARIADB_ROOT_PASSWORD=datarig-root "$image" ${options[@]+"${options[@]}"}
# Over TCP: the image's first, temporary server (it makes the users) listens on its socket only.
for _ in $(seq 1 90); do
  if docker exec "$name" mariadb -h127.0.0.1 -P3306 --protocol=TCP -uroot -pdatarig-root -e 'SELECT 1' >/dev/null 2>&1; then
    break
  fi
  sleep 2
done
docker exec "$name" mariadb -h127.0.0.1 -P3306 --protocol=TCP -uroot -pdatarig-root -N -e 'SELECT VERSION()'
# The server is the one asked for: performance_schema on, or off.
on=$(docker exec "$name" mariadb -h127.0.0.1 -P3306 --protocol=TCP -uroot -pdatarig-root -N -e 'SELECT @@performance_schema')
echo "performance_schema: $on"
want=1
if [ "$ps" = ps=off ]; then
  want=0
fi
if [ "$on" != "$want" ]; then
  echo "performance_schema is $on, wanted $ps" >&2
  exit 1
fi
for f in dev/init-mysql/*.sql; do
  echo "loading $f"
  docker exec -i "$name" mariadb -uroot -pdatarig-root --default-character-set=utf8mb4 < "$f"
done
