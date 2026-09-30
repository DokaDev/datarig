#!/usr/bin/env bash
# Writes the Homebrew formula of release <tag> to stdout: the archive for the machine's OS and
# architecture (macOS and Linux, arm64 and x86_64), downloaded from <base-url>, with the
# checksums of the archives in <dist> (release-archive.sh's .sha256 files).
# Usage: homebrew-formula.sh <tag> <base-url> <dist>
set -euo pipefail

tag=$1
base=$2
dist=$3

sha() {
  local file=$dist/datarig-$tag-$1.tar.gz.sha256
  if [[ ! -f $file ]]; then
    echo "no $file" >&2
    exit 1
  fi
  cut -d ' ' -f 1 "$file"
}
mac_arm=$(sha aarch64-apple-darwin)
mac_intel=$(sha x86_64-apple-darwin)
linux_arm=$(sha aarch64-unknown-linux-gnu)
linux_intel=$(sha x86_64-unknown-linux-gnu)

cat << FORMULA
class Datarig < Formula
  desc "Terminal database client with a DataGrip-style workspace"
  homepage "https://github.com/DokaDev/datarig"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    on_arm do
      url "$base/datarig-$tag-aarch64-apple-darwin.tar.gz"
      sha256 "$mac_arm"
    end
    on_intel do
      url "$base/datarig-$tag-x86_64-apple-darwin.tar.gz"
      sha256 "$mac_intel"
    end
  end

  on_linux do
    on_arm do
      url "$base/datarig-$tag-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "$linux_arm"
    end
    on_intel do
      url "$base/datarig-$tag-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "$linux_intel"
    end
  end

  def install
    bin.install "datarig"
    doc.install "README.md", "THIRD-PARTY-NOTICES.html", "licenses"
  end

  test do
    assert_equal "datarig #{version}", shell_output("#{bin}/datarig --version").strip
  end
end
FORMULA
