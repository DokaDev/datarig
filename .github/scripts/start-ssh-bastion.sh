#!/usr/bin/env bash
# Starts the SSH bastion of dev/ssh on the Docker network of a CI job's service containers
# (the first argument), so it reaches `postgres`, `pgbouncer` and `mysql` by name, published on
# 127.0.0.1:52222. Its keys are made here (dev/ssh/make-fixture.sh), never committed;
# DATARIG_SSH_FIXTURE tells the tests where they are.
set -euo pipefail
network=$1
fixture="$RUNNER_TEMP/ssh-fixture"
sh dev/ssh/make-fixture.sh "$fixture"
docker build -q -t datarig-ssh-bastion dev/ssh
docker run -d --name datarig-ssh-bastion --network "$network" \
  -p 127.0.0.1:52222:22 -v "$fixture:/fixture:ro" datarig-ssh-bastion
for _ in $(seq 1 50); do
  if (exec 3<>/dev/tcp/127.0.0.1/52222) 2>/dev/null; then break; fi
  sleep 0.2
done
echo "DATARIG_SSH_FIXTURE=$fixture" >> "$GITHUB_ENV"
