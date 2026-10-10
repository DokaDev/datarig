# The SSH bastion of the tests: OpenSSH on Alpine, reached from the host on 127.0.0.1:52222,
# which can reach `postgres`, `pgbouncer` and `mysql` on the compose network. Keys come from a
# fixture made at test time (make-fixture.sh), mounted at /fixture; none is baked in.
FROM alpine:3.22
RUN apk add --no-cache openssh-server \
    && adduser -D -s /sbin/nologin tunnel \
    && adduser -D -s /sbin/nologin pw \
    && echo "tunnel:*" | chpasswd -e \
    && echo "pw:datarig-pw" | chpasswd \
    && mkdir -p /home/tunnel/.ssh && chown tunnel:tunnel /home/tunnel/.ssh && chmod 700 /home/tunnel/.ssh
COPY sshd_config /etc/ssh/sshd_config
COPY entrypoint.sh /entrypoint.sh
EXPOSE 22
ENTRYPOINT ["/bin/sh", "/entrypoint.sh"]
