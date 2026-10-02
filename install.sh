#!/bin/sh
# Adapted from ms-todo install.sh @ 72a406042a4d46c2f8cc7c3afe6bda12e11f923b
# (POSIX sh, so `curl -fsSL .../install.sh | sh` works where sh is dash;
# sha256 checked before anything is installed). Changes: macOS only, a
# --prefix, a local --archive, and a warning when another `chatgpt` earlier
# on PATH would run instead (the retired TS CLI's `bun link` left one in
# ~/.bun/bin).
set -eu

repo="planetaryescape/chatgpt-cli"
prefix="${CHATGPT_INSTALL_PREFIX:-$HOME/.local}"
version="${CHATGPT_VERSION:-latest}"
archive_file=""

usage() {
  cat <<'EOF'
usage: install.sh [--version <version>] [--prefix <dir>] [--archive <file>]

Installs the chatgpt release archive for this Mac into <prefix>/bin, after
checking it against the .sha256 file published with it.

  --version <v>     Release version, e.g. v0.1.0. Defaults to the latest.
  --prefix <dir>    Install into <dir>/bin. Defaults to ~/.local.
  --archive <file>  Install a local chatgpt-v<ver>-macos-<arch>.tar.gz
                    (with <file>.sha256 beside it) instead of downloading.

Environment: CHATGPT_VERSION, CHATGPT_INSTALL_PREFIX.
EOF
}

while [ $# -gt 0 ]; do
  case "$1" in
    --version)
      [ $# -ge 2 ] || { echo "--version requires a value" >&2; exit 64; }
      version="$2"
      shift 2
      ;;
    --prefix)
      [ $# -ge 2 ] || { echo "--prefix requires a value" >&2; exit 64; }
      prefix="$2"
      shift 2
      ;;
    --archive)
      [ $# -ge 2 ] || { echo "--archive requires a value" >&2; exit 64; }
      archive_file="$2"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "unknown argument: $1" >&2
      usage >&2
      exit 64
      ;;
  esac
done

need() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "$1 is required" >&2
    exit 69
  fi
}

need tar
need shasum

case "$(uname -s):$(uname -m)" in
  Darwin:arm64) arch="aarch64" ;;
  Darwin:x86_64) arch="x86_64" ;;
  *)
    echo "no prebuilt chatgpt for $(uname -s) $(uname -m): it reads browser cookies on macOS only" >&2
    exit 69
    ;;
esac

tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT INT TERM

if [ -n "$archive_file" ]; then
  [ -f "$archive_file" ] || { echo "no archive at $archive_file" >&2; exit 66; }
  [ -f "$archive_file.sha256" ] || { echo "no checksum at $archive_file.sha256" >&2; exit 66; }
  archive="$(basename "$archive_file")"
  cp "$archive_file" "$tmpdir/$archive"
  cp "$archive_file.sha256" "$tmpdir/$archive.sha256"
else
  need curl
  if [ "$version" = "latest" ]; then
    latest_url="$(curl -fsSIL -o /dev/null -w '%{url_effective}' "https://github.com/$repo/releases/latest")"
    version="${latest_url##*/}"
    case "$version" in
      v[0-9]*) ;;
      *)
        echo "could not find the latest chatgpt release (got $latest_url)" >&2
        exit 69
        ;;
    esac
  fi
  tag="v${version#v}"
  archive="chatgpt-${tag}-macos-${arch}.tar.gz"
  base_url="https://github.com/$repo/releases/download/$tag"
  curl -fL --proto '=https' --tlsv1.2 -o "$tmpdir/$archive" "$base_url/$archive"
  curl -fL --proto '=https' --tlsv1.2 -o "$tmpdir/$archive.sha256" "$base_url/$archive.sha256"
fi

(cd "$tmpdir" && shasum -a 256 -c "$archive.sha256")

mkdir -p "$tmpdir/unpacked"
tar -xzf "$tmpdir/$archive" -C "$tmpdir/unpacked"
if [ ! -x "$tmpdir/unpacked/chatgpt" ]; then
  echo "the archive has no executable chatgpt" >&2
  exit 65
fi

bin_dir="$prefix/bin"
mkdir -p "$bin_dir"
install -m 0755 "$tmpdir/unpacked/chatgpt" "$bin_dir/chatgpt"
# Its own command, so `set -e` stops on a binary that can't run here.
installed_version="$("$bin_dir/chatgpt" --version)"
echo "installed $installed_version to $bin_dir/chatgpt"

# The first `chatgpt` on PATH is the one a shell runs.
first=""
old_ifs="$IFS"
IFS=:
for dir in $PATH; do
  if [ -n "$dir" ] && [ -x "$dir/chatgpt" ] && [ ! -d "$dir/chatgpt" ]; then
    first="$dir/chatgpt"
    break
  fi
done
IFS="$old_ifs"

case ":$PATH:" in
  *":$bin_dir:"*)
    if [ -n "$first" ] && [ "$first" != "$bin_dir/chatgpt" ]; then
      echo "warning: $first comes earlier on your PATH, so \`chatgpt\` still runs it, not $bin_dir/chatgpt." >&2
      echo "         Remove it (for the retired TS CLI's link: rm \"$first\"), or put $bin_dir earlier on PATH." >&2
    fi
    ;;
  *)
    echo "note: $bin_dir is not on your PATH; add it, ahead of any other chatgpt." >&2
    if [ -n "$first" ]; then
      echo "      Until then \`chatgpt\` runs $first." >&2
    fi
    ;;
esac
