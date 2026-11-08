#!/usr/bin/env bash
# Renders the README recordings: every docs/demo/tapes/*.tape (or the ones named) with VHS,
# into <out-dir> as <name>.gif plus the stills the tapes take (<name>.png), and social.png
# (the repository's social preview) from social.tape.
#   docs/demo/render.sh <out-dir> [tape-name...]
# Needs vhs, ttyd, ffmpeg, Python 3 with Pillow (for social.png) and JetBrainsMono Nerd Font (.github/scripts/demo-tools.sh installs
# them on CI), the demo databases of docs/demo/compose.yml with the certificates of
# make-certs.sh (DATARIG_DEMO_CERTS, default docs/demo/.certs), and datarig on the PATH (or
# DATARIG_BIN, the binary to record). In each tape datarig runs with docs/demo/config in a
# scratch configuration, state and data directory of its own and an in-memory secret store, so
# nothing outside that directory is read or written. A tape whose Wait does not match the screen fails
# the run: the recordings never show a screen datarig no longer has. A tape with a load
# (docs/demo/load/<name>.sh, which the demo-load service of compose.yml runs against the demo
# databases) also needs docker: the load runs only while that tape records.
set -euo pipefail

if [ $# -lt 1 ]; then
  echo "usage: $0 <out-dir> [tape-name...]" >&2
  exit 2
fi
demo=$(cd "$(dirname "$0")" && pwd)
mkdir -p "$1"
out=$(cd "$1" && pwd)
shift

real=${DATARIG_BIN:-$(command -v datarig || true)}
[ -n "$real" ] && [ -x "$real" ] || { echo "datarig is not on the PATH (or set DATARIG_BIN)" >&2; exit 1; }
real=$(cd "$(dirname "$real")" && pwd)/$(basename "$real")
for tool in vhs ttyd ffmpeg python3; do
  command -v "$tool" >/dev/null || { echo "$tool is not on the PATH" >&2; exit 1; }
done
"$real" --version
certs=${DATARIG_DEMO_CERTS:-$demo/.certs}
[ -f "$certs/ca.crt" ] || { echo "no demo CA in $certs (docs/demo/make-certs.sh)" >&2; exit 1; }
ca=$(cd "$certs" && pwd)/ca.crt

if [ $# -eq 0 ]; then
  for tape in "$demo"/tapes/*.tape; do
    name=$(basename "$tape" .tape)
    [ "$name" = settings ] || set -- "$@" "$name"
  done
fi

scratch=$(mktemp -d)
trap 'load_stop || true; rm -rf "$scratch"' EXIT

# The load of a tape (load/<name>.sh): the demo-load service, started with no other service of
# the demo project touched, and taken as running only once it says so.
load() {
  docker compose -f "$demo/compose.yml" --profile load "$@"
}
loaded=
load_start() {
  [ -f "$demo/load/$1.sh" ] || return 0
  command -v docker >/dev/null || { echo "the $1 tape has a load, and docker is not on the PATH" >&2; return 1; }
  loaded=$1
  DATARIG_DEMO_LOAD=$1 load up -d --no-deps --force-recreate demo-load || return 1
  for _ in $(seq 120); do
    docker logs datarig-demo-load 2>&1 | grep -qx 'demo load running' && return 0
    sleep 0.5
  done
  echo "the load of $1 did not start:" >&2
  docker logs datarig-demo-load >&2 || true
  return 1
}
# Removes the load and waits until the demo PostgreSQL has no session of its client left, so
# the next tape records an idle server.
load_stop() {
  [ -n "$loaded" ] || return 0
  local addr left
  addr=$(docker inspect -f '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}' datarig-demo-load) || addr=
  load rm -s -f demo-load || return 1
  loaded=
  [ -n "$addr" ] || return 0
  for _ in $(seq 30); do
    left=$(docker exec -e PGPASSWORD=demo datarig-demo-pg psql -h 127.0.0.1 -U demo -d acme -XtAc \
      "SELECT count(*) FROM pg_stat_activity WHERE client_addr = '$addr'") || return 1
    [ "$left" = 0 ] && return 0
    sleep 0.5
  done
  echo "$left sessions of the load are still open" >&2
  return 1
}

# record <name> <run dir>: VHS records the tape (with its load, if it has one), its output also
# in <run dir>/vhs.log.
record() {
  local status=0
  load_start "$1" || { load_stop || true; return 1; }
  (
    cd "$2/work"
    export PATH="$2/bin:$PATH" LANG=C.UTF-8 TZ=UTC
    vhs "$1.tape" -o "$out/$1.gif" 2>&1 | tee "$2/vhs.log"
  ) || status=$?
  load_stop || status=1
  return $status
}

# Whether the recording failed because VHS's headless browser did not start: VHS gives it 30 s
# to open its debugging port (on a port VHS picked and let go of just before) and on the hosted
# runners it sometimes does not. Nothing of the tape ran then, so such a recording is tried once
# more; a tape whose Wait does not match the screen is never tried again.
browser_did_not_start() {
  grep -q -E 'could not (start|connect to) browser|browser debugging endpoint|browser exited unexpectedly' "$1/vhs.log" &&
    echo "VHS's browser did not start; trying the tape once more"
}
failed=()
for name in "$@"; do
  [ -f "$demo/tapes/$name.tape" ] || { echo "no tape $name" >&2; exit 2; }
  run=$scratch/$name
  mkdir -p "$run/work" "$run/bin" "$run/config/datarig" "$run/state" "$run/data" "$run/tmp"
  # This version already ran here: What's new does not open over the recording.
  mkdir -p "$run/state/datarig"
  echo "seen = \"$("$real" --version | sed 's/^datarig //')\"" > "$run/state/datarig/update.toml"
  sed "s|\"DEMO_CA\"|\"$ca\"|" "$demo/config/config.toml" > "$run/config/datarig/config.toml"
  cp "$demo/tapes/$name.tape" "$demo/tapes/settings.tape" "$run/work/"
  # The tape's `datarig`: the scratch directories are datarig's alone (VHS's browser keeps the
  # usual ones, where it finds the font).
  cat > "$run/bin/datarig" <<WRAPPER
#!/bin/sh
export DATARIG_SECRET_STORE=memory DATARIG_DEMO_PASSWORD=demo DATARIG_NO_UPDATE_CHECK=1
export XDG_CONFIG_HOME='$run/config' XDG_STATE_HOME='$run/state' XDG_DATA_HOME='$run/data' TMPDIR='$run/tmp'
unset DATARIG_STATE_DIR DATARIG_DATA_DIR TMUX
exec '$real' "\$@"
WRAPPER
  chmod +x "$run/bin/datarig"
  echo "::group::$name"
  if record "$name" "$run" || { browser_did_not_start "$run" && record "$name" "$run"; }; then
    for png in "$run/work"/*.png; do
      [ -e "$png" ] && mv "$png" "$out/"
    done
  else
    failed+=("$name")
  fi
  echo "::endgroup::"
done

# The social preview (social.py): the still of social.tape under a band with the name and the
# tagline. Its GIF is of no use.
if [ -f "$out/social-raw.png" ]; then
  bold=$(fc-match -f '%{file}' 'JetBrainsMono Nerd Font:style=Bold')
  regular=$(fc-match -f '%{file}' 'JetBrainsMono Nerd Font:style=Regular')
  python3 "$demo/social.py" "$out/social-raw.png" "$out/social.png" "$bold" "$regular"
  rm -f "$out/social-raw.png" "$out/social.gif"
fi

ls -l "$out"
if [ ${#failed[@]} -gt 0 ]; then
  echo "These recordings no longer match datarig: ${failed[*]}" >&2
  exit 1
fi
