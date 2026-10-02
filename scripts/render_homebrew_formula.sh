#!/usr/bin/env bash
# Adapted from ms-todo scripts/render_homebrew_formula.sh @ 72a406042a4d46c2f8cc7c3afe6bda12e11f923b
# Changes: chatgpt's archive names and template; the two macOS archives
# only.
#
# Renders the Homebrew formula for a release from its .sha256 files.

set -euo pipefail

if [[ $# -ne 3 ]]; then
  echo "usage: $0 <version> <checksums-dir> <output-formula>" >&2
  exit 64
fi

version="${1#v}"
checksums_dir="$2"
output_formula="$3"
template="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/packaging/homebrew/chatgpt.rb"

require_checksum() {
  local archive="$1"
  local file="$checksums_dir/$archive.sha256"
  if [[ ! -f "$file" ]]; then
    echo "missing checksum file: $file" >&2
    exit 1
  fi
  local sha
  sha="$(awk '{print $1; exit}' "$file")"
  if [[ ! "$sha" =~ ^[0-9a-f]{64}$ ]]; then
    echo "not a sha256 in $file: $sha" >&2
    exit 1
  fi
  printf '%s' "$sha"
}

macos_aarch64_sha="$(require_checksum "chatgpt-v${version}-macos-aarch64.tar.gz")"
macos_x86_64_sha="$(require_checksum "chatgpt-v${version}-macos-x86_64.tar.gz")"

mkdir -p "$(dirname "$output_formula")"

sed \
  -e "s/__VERSION__/${version}/g" \
  -e "s/__SHA256_MACOS_AARCH64__/${macos_aarch64_sha}/g" \
  -e "s/__SHA256_MACOS_X86_64__/${macos_x86_64_sha}/g" \
  "$template" > "$output_formula"
