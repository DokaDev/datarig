#!/usr/bin/env bash
# The tools that record the README demos (docs/demo/render.sh) on x86_64 Linux: VHS, ttyd and
# ffmpeg, which VHS drives, and JetBrainsMono Nerd Font, the font every recording uses (the
# explorer's icons are Nerd Font glyphs). VHS, ttyd and the font are release downloads pinned to
# a version and to the SHA-256 of the file (from the GitHub release's asset digest, ttyd's from
# its SHA256SUMS); a download that does not match fails here. ffmpeg and a CJK font come from
# apt. CI tools
# only, never dependencies of datarig. VHS and ttyd go to $RUNNER_TEMP/bin, which goes on the
# PATH of the steps after this one; the font to ~/.local/share/fonts.
# To bump one: change its version and its digest together.
set -euo pipefail

vhs_version=0.12.1
vhs_sha256=eb33787a5b1cebf91fe0b18bd231e2d71d36c1ff4babd99ddac1a6ab411efd10
ttyd_version=1.7.7
ttyd_sha256=8a217c968aba172e0dbf3f34447218dc015bc4d5e59bf51db2f2cd12b7be4f55
nerd_fonts_version=v3.5.1
font_sha256=04d5e8f903693f9dd13e16f867e994834e681eb3c72c0d337a770dcda09010cf

if [ "$(uname -sm)" != "Linux x86_64" ]; then
  echo "demo-tools.sh downloads the x86_64 Linux builds; this is $(uname -sm)" >&2
  exit 1
fi

bin=${RUNNER_TEMP:?}/bin
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$bin"

# fetch <url> <sha256>: the file in $work, checked.
fetch() {
  local file=$work/${1##*/}
  curl -sSfL --retry 3 --retry-all-errors --retry-delay 2 -o "$file" "$1"
  echo "$2  $file" | sha256sum -c -
}

fetch "https://github.com/charmbracelet/vhs/releases/download/v$vhs_version/vhs_${vhs_version}_Linux_x86_64.tar.gz" \
  "$vhs_sha256"
tar -xzf "$work/vhs_${vhs_version}_Linux_x86_64.tar.gz" -C "$work"
install -m 0755 "$work/vhs_${vhs_version}_Linux_x86_64/vhs" "$bin/vhs"

fetch "https://github.com/tsl0922/ttyd/releases/download/$ttyd_version/ttyd.x86_64" "$ttyd_sha256"
install -m 0755 "$work/ttyd.x86_64" "$bin/ttyd"

fetch "https://github.com/ryanoasis/nerd-fonts/releases/download/$nerd_fonts_version/JetBrainsMono.tar.xz" \
  "$font_sha256"
fonts=$HOME/.local/share/fonts/JetBrainsMonoNerdFont
mkdir -p "$fonts"
tar -xJf "$work/JetBrainsMono.tar.xz" -C "$fonts" --wildcards 'JetBrainsMonoNerdFont-*.ttf'
fc-cache -f "$fonts" >/dev/null
fc-list | grep -q 'JetBrainsMono Nerd Font' || { echo "JetBrainsMono Nerd Font is not installed" >&2; exit 1; }

# ffmpeg, and a CJK font for the few full-width signs datarig draws (U+FF0B, the plus of the
# explorer's first line), which the browser takes from any installed font that has them.
sudo apt-get update -qq
sudo apt-get install -y -qq --no-install-recommends ffmpeg fonts-noto-cjk >/dev/null
fc-list ':charset=ff0b' family | grep -q . || { echo "no installed font has U+FF0B" >&2; exit 1; }

if [ -n "${GITHUB_PATH:-}" ]; then
  echo "$bin" >> "$GITHUB_PATH"
fi
"$bin/vhs" --version
"$bin/ttyd" --version
ffmpeg -version | head -n 1
