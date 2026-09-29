#!/bin/sh
# sshd refuses host keys that root does not own or others can read: copy the fixture's.
set -e
for k in ssh_host_ed25519_key ssh_host_rsa_key; do
    cp "/fixture/$k" "/etc/ssh/$k"
    cp "/fixture/$k.pub" "/etc/ssh/$k.pub"
    chown root:root "/etc/ssh/$k" && chmod 600 "/etc/ssh/$k"
done
cp /fixture/authorized_keys /home/tunnel/.ssh/authorized_keys
chown tunnel:tunnel /home/tunnel/.ssh/authorized_keys && chmod 600 /home/tunnel/.ssh/authorized_keys
exec /usr/sbin/sshd -D -e
