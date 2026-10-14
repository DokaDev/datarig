#!/bin/sh
# Removes the scratch TLS servers of up.sh.
docker rm -f zz_datarig_pg_tls zz_datarig_mysql_tls >/dev/null 2>&1 || true
