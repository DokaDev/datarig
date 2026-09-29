#!/bin/sh
# Keys for the SSH bastion of the tests, made fresh, never committed:
#   ssh_host_{ed25519,rsa}_key[.pub]  the bastion's host keys
#   id_ed25519                        a client key (OpenSSH format)
#   id_rsa.pem                        a client key as AWS hands them out (PEM, PKCS#1 RSA)
#   authorized_keys                   both client keys, for the user `tunnel`
# Usage: make-fixture.sh [dir]   (default: dev/ssh/.fixture, git-ignored)
set -eu
dir=${1:-$(dirname "$0")/.fixture}
mkdir -p "$dir"
cd "$dir"
rm -f ssh_host_* id_ed25519* id_rsa.pem* authorized_keys
ssh-keygen -q -t ed25519 -N "" -C bastion -f ssh_host_ed25519_key
ssh-keygen -q -t rsa -b 3072 -N "" -C bastion -f ssh_host_rsa_key
ssh-keygen -q -t ed25519 -N "" -C datarig-test -f id_ed25519
ssh-keygen -q -t rsa -b 2048 -m PEM -N "" -C datarig-test -f id_rsa.pem
cat id_ed25519.pub id_rsa.pem.pub > authorized_keys
chmod 600 id_ed25519 id_rsa.pem ssh_host_ed25519_key ssh_host_rsa_key
chmod 644 authorized_keys ./*.pub
pwd
