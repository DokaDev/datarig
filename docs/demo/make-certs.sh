#!/bin/sh
# The demo databases' TLS certificates, made fresh and never committed (throwaway keys), so the
# recordings show connections whose server is verified:
#   ca.crt                  the demo CA (the profiles' sslrootcert)
#   server.crt, server.key  the servers' certificate, for 127.0.0.1 and localhost
# Usage: make-certs.sh [dir]   (default: docs/demo/.certs, git-ignored). Needs OpenSSL 1.1.1+.
set -eu
dir=${1:-$(dirname "$0")/.certs}
mkdir -p "$dir"
cd "$dir"
rm -f ca.crt ca.key server.crt server.key server.csr server.cnf ca.srl
openssl req -x509 -newkey rsa:2048 -nodes -days 7 -subj "/CN=datarig demo CA" \
    -addext basicConstraints=critical,CA:TRUE -addext keyUsage=critical,keyCertSign,cRLSign \
    -keyout ca.key -out ca.crt 2>/dev/null
printf '%s\n' "subjectAltName=IP:127.0.0.1,DNS:localhost" "extendedKeyUsage=serverAuth" > server.cnf
openssl req -new -newkey rsa:2048 -nodes -subj "/CN=localhost" -keyout server.key -out server.csr 2>/dev/null
openssl x509 -req -in server.csr -CA ca.crt -CAkey ca.key -CAcreateserial -days 7 \
    -extfile server.cnf -out server.crt 2>/dev/null
rm -f ca.key ca.srl server.csr server.cnf
# Readable by the servers' users in the containers (they copy the key and lock it down).
chmod 644 ./*.crt server.key
pwd
