#!/usr/bin/env bash
# The tools that record the README demos (docs/demo/render.sh) on x86_64 Linux: VHS, ttyd and
# ffmpeg, which VHS drives, and JetBrainsMono Nerd Font, the font every recording uses (the
# explorer's icons are Nerd Font glyphs), with a CJK font as its fallback. VHS, ttyd and the
# Nerd Font are release downloads pinned to a version and to the SHA-256 of the file (from the
# GitHub release's asset digest, ttyd's from its SHA256SUMS); the CJK font is pinned to a commit
# of its repository and its SHA-256; ffmpeg is a static build, pinned the same way. A download
# that does not match fails here. CI tools only, never dependencies of datarig. VHS, ttyd and
# ffmpeg go to $RUNNER_TEMP/bin, which goes on the PATH of the steps after this one; the fonts to
# ~/.local/share/fonts.
# To bump one: change its version and its digest together.
set -euo pipefail

vhs_version=0.12.1
vhs_sha256=eb33787a5b1cebf91fe0b18bd231e2d71d36c1ff4babd99ddac1a6ab411efd10
ttyd_version=1.7.7
ttyd_sha256=8a217c968aba172e0dbf3f34447218dc015bc4d5e59bf51db2f2cd12b7be4f55
nerd_fonts_version=v3.5.1
font_sha256=04d5e8f903693f9dd13e16f867e994834e681eb3c72c0d337a770dcda09010cf
noto_cjk_commit=f8d157532fbfaeda587e826d4cd5b21a49186f7c
cjk_sha256=ec04cc376b34887cedbdf84074e2e226ed2761eeabdcb9173fc1dd7bfd153ef7
ffmpeg_release=b6.1.1
ffmpeg_sha256=bfe8a8fc511530457b528c48d77b5737527b504a3797a9bc4866aeca69c2dffa

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

# A CJK font for the few full-width signs datarig draws (U+FF0B, the plus of the explorer's
# first line), which the browser takes from any installed font that has them.
fetch "https://raw.githubusercontent.com/notofonts/noto-cjk/$noto_cjk_commit/Sans/Mono/NotoSansMonoCJKsc-Regular.otf" \
  "$cjk_sha256"
install -m 0644 "$work/NotoSansMonoCJKsc-Regular.otf" "$fonts/"
fc-cache -f "$fonts" >/dev/null
fc-list : family | grep -F 'JetBrainsMono Nerd Font' >/dev/null || { echo "JetBrainsMono Nerd Font is not installed" >&2; exit 1; }
fc-list ':charset=ff0b' family | grep . >/dev/null || { echo "no installed font has U+FF0B" >&2; exit 1; }

# A static ffmpeg (7.0.2): installing it with apt took from under a minute to half an hour on the hosted runners.
fetch "https://github.com/eugeneware/ffmpeg-static/releases/download/$ffmpeg_release/ffmpeg-linux-x64.gz" \
  "$ffmpeg_sha256"
gunzip -c "$work/ffmpeg-linux-x64.gz" > "$bin/ffmpeg"
chmod 0755 "$bin/ffmpeg"

# Pillow for the social preview (docs/demo/social.py), pinned by the hashes of its wheels, in
# a virtual environment whose python3 goes on the PATH.
python3 -m venv "$RUNNER_TEMP/py"
cat > "$work/requirements.txt" <<'REQUIREMENTS'
pillow==11.3.0 \
    --hash=sha256:67172f2944ebba3d4a7b54f2e95c786a3a50c21b88456329314caaa28cda70f6 \
    --hash=sha256:676b2815362456b5b3216b4fd5bd89d362100dc6f4945154ff172e206a22c024
REQUIREMENTS
"$RUNNER_TEMP/py/bin/pip" install -q --only-binary=:all: --require-hashes -r "$work/requirements.txt"

if [ -n "${GITHUB_PATH:-}" ]; then
  echo "$bin" >> "$GITHUB_PATH"
  echo "$RUNNER_TEMP/py/bin" >> "$GITHUB_PATH"
fi
"$bin/vhs" --version
"$bin/ttyd" --version
"$bin/ffmpeg" -version | sed -n 1p
