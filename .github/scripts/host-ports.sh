#!/usr/bin/env bash
# Checks that every host port a CI job publishes (the arguments) lies below the runner's range
# of ephemeral ports (net.ipv4.ip_local_port_range, 32768-60999 on the hosted runners).
# A port inside that range can be taken at any moment by an outgoing connection the kernel
# gives a local port from it: pulling a service image opens many, and Docker then fails to
# start the next container with "failed to bind host port ...: address already in use".
# The service containers start before the first step, so this cannot prevent such a failure;
# it fails a job whose ports were moved back into the range, with the reason.
#   host-ports.sh <port>...
set -euo pipefail
read -r low high < /proc/sys/net/ipv4/ip_local_port_range
echo "ephemeral ports of this runner: $low-$high"
bad=()
for port in "$@"; do
  if [ "$port" -ge "$low" ] && [ "$port" -le "$high" ]; then
    bad+=("$port")
  fi
done
if [ ${#bad[@]} -gt 0 ]; then
  echo "host ports inside the ephemeral range ($low-$high): ${bad[*]}; pick ports below $low" >&2
  exit 1
fi
