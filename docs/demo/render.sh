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
# the run: the recordings never show a screen datarig no longer has.
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
trap 'rm -rf "$scratch"' EXIT
failed=()
for name in "$@"; do
  [ -f "$demo/tapes/$name.tape" ] || { echo "no tape $name" >&2; exit 2; }
  run=$scratch/$name
  mkdir -p "$run/work" "$run/bin" "$run/config/datarig" "$run/state" "$run/data" "$run/tmp"
  sed "s|\"DEMO_CA\"|\"$ca\"|" "$demo/config/config.toml" > "$run/config/datarig/config.toml"
  cp "$demo/tapes/$name.tape" "$demo/tapes/settings.tape" "$run/work/"
  # The tape's `datarig`: the scratch directories are datarig's alone (VHS's browser keeps the
  # usual ones, where it finds the font).
  cat > "$run/bin/datarig" <<WRAPPER
#!/bin/sh
export DATARIG_SECRET_STORE=memory DATARIG_DEMO_PASSWORD=demo
export XDG_CONFIG_HOME='$run/config' XDG_STATE_HOME='$run/state' XDG_DATA_HOME='$run/data' TMPDIR='$run/tmp'
unset DATARIG_STATE_DIR DATARIG_DATA_DIR TMUX
exec '$real' "\$@"
WRAPPER
  chmod +x "$run/bin/datarig"
  echo "::group::$name"
  if (
    cd "$run/work"
    export PATH="$run/bin:$PATH" LANG=C.UTF-8 TZ=UTC
    vhs "$name.tape" -o "$out/$name.gif"
  ); then
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
