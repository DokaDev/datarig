#!/bin/sh
# Certificates for the TLS tests, made fresh, never committed (throwaway keys):
#   ca.crt                    the CA the servers' certificates are signed by
#   server.crt, server.key    the servers' certificate: localhost, pg-tls.test, mysql-tls.test
#                             (not 127.0.0.1: connecting by that address is a name mismatch)
#   other-ca.crt              a CA nothing is signed by (an untrusted server)
#   client-ca.crt             the CA the PostgreSQL server takes client certificates of
#   client.crt, client.key    a client certificate of the user zz_cert, by client-ca
#   stranger.crt, stranger.key  a client certificate of zz_cert by another CA (refused)
# Usage: make-certs.sh [dir]   (default: dev/tls/.fixture, git-ignored)
set -eu
dir=${1:-$(dirname "$0")/.fixture}
mkdir -p "$dir"
cd "$dir"
rm -f ./*.crt ./*.key ./*.csr ./*.srl ./*.cnf
ca() {
    openssl req -x509 -newkey rsa:2048 -nodes -days 30 -subj "/CN=$2" \
        -addext basicConstraints=critical,CA:TRUE -addext keyUsage=critical,keyCertSign,cRLSign \
        -keyout "$1.key" -out "$1.crt" 2>/dev/null
}
# leaf <name> <ca> <cn> <extensions>
leaf() {
    printf '%s\n' "$4" > "$1.cnf"
    openssl req -new -newkey rsa:2048 -nodes -subj "/CN=$3" -keyout "$1.key" -out "$1.csr" 2>/dev/null
    openssl x509 -req -in "$1.csr" -CA "$2.crt" -CAkey "$2.key" -CAcreateserial -days 30 \
        -extfile "$1.cnf" -out "$1.crt" 2>/dev/null
    rm -f "$1.csr" "$1.cnf"
}
ca ca "datarig test CA"
ca other-ca "datarig other CA"
ca client-ca "datarig client CA"
leaf server ca localhost "subjectAltName=DNS:localhost,DNS:pg-tls.test,DNS:mysql-tls.test
extendedKeyUsage=serverAuth"
leaf client client-ca zz_cert "extendedKeyUsage=clientAuth"
leaf stranger other-ca zz_cert "extendedKeyUsage=clientAuth"
rm -f ./*.srl other-ca.key
chmod 600 ./*.key
chmod 644 ./*.crt
pwd
