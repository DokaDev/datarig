#!/usr/bin/env bash
# Every recording the README and the docs site show from https://dokadev.github.io/datarig/demo/
# must be one a tape makes (docs/demo/render.sh): <name>.gif needs docs/demo/tapes/<name>.tape,
# <name>.png a tape that takes `Screenshot <name>.png`, and social.png comes from social.tape.
# Otherwise the page links to a file the release never publishes.
set -euo pipefail
cd "$(dirname "$0")/../.."

tapes=docs/demo/tapes
missing=0
while IFS=: read -r file name; do
  case $name in
    social.png) made=$tapes/social.tape ;;
    *.gif) made=$tapes/${name%.gif}.tape ;;
    *.png) made=$(grep -lxF "Screenshot $name" "$tapes"/*.tape | head -n 1 || true) ;;
  esac
  if [ -z "$made" ] || [ ! -f "$made" ] || [ "$(basename "$made")" = settings.tape ]; then
    echo "$file shows demo/$name, which no tape in $tapes makes" >&2
    missing=1
  fi
done < <(grep -rEo 'https://dokadev\.github\.io/datarig/demo/[A-Za-z0-9_-]+\.(gif|png)' \
           README.md docs/site/src | sed -E 's|^([^:]+):.*/demo/|\1:|' | sort -u)
exit $missing
