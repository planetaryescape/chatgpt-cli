#!/bin/sh
# Adapted from ms-todo install.sh @ 72a406042a4d46c2f8cc7c3afe6bda12e11f923b
# (POSIX sh, so `curl -fsSL .../install.sh | sh` works where sh is dash;
# sha256 checked before anything is installed). Changes: platform selection, a
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

Installs the chatgpt release archive for this OS into <prefix>/bin, after
checking it against the .sha256 file published with it.

  --version <v>     Release version, e.g. v0.1.0. Defaults to the latest.
  --prefix <dir>    Install into <dir>/bin. Defaults to ~/.local.
  --archive <file>  Install a local chatgpt-v<ver>-<os>-<arch>.tar.gz
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
if command -v sha256sum >/dev/null 2>&1; then
  checksum="sha256sum"
elif command -v shasum >/dev/null 2>&1; then
  checksum="shasum"
else
  echo "sha256sum or shasum is required to verify the archive" >&2
  exit 69
fi

os="$(uname -s)"
machine="$(uname -m)"
case "$os:$machine" in
  Darwin:arm64) platform="macos"; arch="aarch64" ;;
  Darwin:x86_64) platform="macos"; arch="x86_64" ;;
  Linux:x86_64) platform="linux"; arch="x86_64" ;;
  *)
    echo "no prebuilt chatgpt for $os $machine: supported platforms are macOS arm64/x86_64 and Linux x86_64 (glibc)" >&2
    exit 69
    ;;
esac

tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT INT TERM

if [ -n "$archive_file" ]; then
  case "$archive_file" in
    /*) ;;
    *) archive_file="$PWD/$archive_file" ;;
  esac
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
  archive="chatgpt-${tag}-${platform}-${arch}.tar.gz"
  base_url="https://github.com/$repo/releases/download/$tag"
  curl -fL --proto '=https' --tlsv1.2 -o "$tmpdir/$archive" "$base_url/$archive"
  curl -fL --proto '=https' --tlsv1.2 -o "$tmpdir/$archive.sha256" "$base_url/$archive.sha256"
fi

# Some sha256sum implementations accept an empty manifest as success. Require
# a digest for this archive before asking the checker to verify its contents.
digest=""
checksum_archive=""
read -r digest checksum_archive < "$tmpdir/$archive.sha256" || true
case "$digest" in
  *[!0-9a-fA-F]*|"")
    echo "invalid SHA-256 checksum for $archive" >&2
    exit 65
    ;;
esac
if [ "${#digest}" -ne 64 ]; then
  echo "invalid SHA-256 checksum for $archive" >&2
  exit 65
fi
case "$checksum_archive" in
  "$archive"|"./$archive"|"*$archive"|"*./$archive") ;;
  *)
    echo "the checksum does not name $archive" >&2
    exit 65
    ;;
esac

if [ "$checksum" = "sha256sum" ]; then
  (cd "$tmpdir" && sha256sum -c "./$archive.sha256")
else
  (cd "$tmpdir" && shasum -a 256 -c "./$archive.sha256")
fi

mkdir -p "$tmpdir/unpacked"
tar -xzf "$tmpdir/$archive" -C "$tmpdir/unpacked"
if [ ! -x "$tmpdir/unpacked/chatgpt" ]; then
  echo "the archive has no executable chatgpt" >&2
  exit 65
fi

case "$prefix" in
  /*) ;;
  *) prefix="$PWD/$prefix" ;;
esac
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
      # Only the retired TS CLI's bun link is known to be safe to delete;
      # anything else (another install, someone else's tool) stays.
      link_target=""
      if [ -L "$first" ]; then
        link_target="$(readlink "$first")"
      fi
      case "$link_target" in
        *node_modules/chatgpt-cli/*)
          echo "         It's the retired TS CLI's bun link; remove it with: rm \"$first\"" >&2
          ;;
        *)
          echo "         Put $bin_dir earlier on PATH than $(dirname "$first")." >&2
          ;;
      esac
    fi
    ;;
  *)
    echo "note: $bin_dir is not on your PATH; add it, ahead of any other chatgpt." >&2
    if [ -n "$first" ]; then
      echo "      Until then \`chatgpt\` runs $first." >&2
    fi
    ;;
esac
