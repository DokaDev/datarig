#!/bin/sh
# Scratch TLS servers for the tests (never the dev services or their volumes): PostgreSQL 17 on
# 55433 and MySQL 8.4 on 53307, with the certificates of make-certs.sh, on the dev compose
# network (so the SSH bastion reaches them as pg-tls.test and mysql-tls.test). Throwaway: no
# volumes; `down.sh` removes them.
#   PostgreSQL: TLS on, every login over TLS with a password (SCRAM), except zz_cert, which logs
#   in with its client certificate; client certificates of client-ca are asked for.
#   MySQL: require_secure_transport=ON (an unencrypted login is refused).
# Usage: up.sh <certs dir> [network]
set -eu
certs=$(cd "$1" && pwd)
net=${2:-datarig-dev_default}
docker run -d --name zz_datarig_pg_tls --network "$net" --network-alias pg-tls.test \
    -p 127.0.0.1:55433:5432 -e POSTGRES_USER=datarig -e POSTGRES_PASSWORD=datarig -e POSTGRES_DB=datarig \
    -e POSTGRES_HOST_AUTH_METHOD=scram-sha-256 -v "$certs:/src:ro" --entrypoint sh postgres:17 -c '
        mkdir -p /certs && cp /src/server.crt /src/server.key /src/client-ca.crt /certs/ &&
        chown postgres /certs/* && chmod 600 /certs/server.key &&
        printf "%s\n" "local all all trust" "hostssl all zz_cert all cert" \
            "hostssl all all all scram-sha-256" > /certs/pg_hba.conf && chown postgres /certs/pg_hba.conf &&
        printf "%s\n" "CREATE ROLE zz_cert LOGIN;" > /docker-entrypoint-initdb.d/zz_cert.sql &&
        exec docker-entrypoint.sh postgres -c ssl=on -c ssl_cert_file=/certs/server.crt \
            -c ssl_key_file=/certs/server.key -c ssl_ca_file=/certs/client-ca.crt -c hba_file=/certs/pg_hba.conf'
docker run -d --name zz_datarig_mysql_tls --network "$net" --network-alias mysql-tls.test \
    -p 127.0.0.1:53307:3306 -e MYSQL_USER=datarig -e MYSQL_PASSWORD=datarig -e MYSQL_DATABASE=datarig \
    -e MYSQL_ROOT_PASSWORD=datarig-root -v "$certs:/src:ro" --entrypoint sh mysql:8.4 -c '
        mkdir -p /certs && cp /src/ca.crt /src/server.crt /src/server.key /certs/ &&
        chown mysql /certs/* && chmod 600 /certs/server.key &&
        exec docker-entrypoint.sh mysqld --require-secure-transport=ON --ssl-ca=/certs/ca.crt \
            --ssl-cert=/certs/server.crt --ssl-key=/certs/server.key'
for i in $(seq 1 90); do
    pg=$(docker exec zz_datarig_pg_tls pg_isready -h 127.0.0.1 -U datarig >/dev/null 2>&1 && echo ok || true)
    my=$(docker exec zz_datarig_mysql_tls mysql -uroot -pdatarig-root --ssl-mode=REQUIRED -e 'SELECT 1' >/dev/null 2>&1 && echo ok || true)
    [ "$pg$my" = okok ] && exit 0
    sleep 2
done
echo "the TLS servers did not come up" >&2
exit 1
