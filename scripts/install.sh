#!/usr/bin/env bash
# Install the ExecScope binary from a GitHub release, verifying its checksum.
#
#   curl -fsSL https://raw.githubusercontent.com/n0ctx/execscope/main/scripts/install.sh | sh
#   ./install.sh --version 0.2.0 --prefix /usr/local/bin
#
# Environment: EXECSCOPE_VERSION, EXECSCOPE_PREFIX, EXECSCOPE_TARGET.
set -euo pipefail

REPO="n0ctx/execscope"
VERSION="${EXECSCOPE_VERSION:-}"
PREFIX="${EXECSCOPE_PREFIX:-$HOME/.local/bin}"
TARGET="${EXECSCOPE_TARGET:-}"
DRY_RUN=0

usage() {
  cat <<'EOF'
Install ExecScope from a GitHub release.

  --version <x.y.z>   release to install (default: the latest one)
  --prefix <dir>      where the binary goes (default: ~/.local/bin)
  --target <triple>   override the detected platform, e.g. x86_64-apple-darwin
  --dry-run           print what would be downloaded and installed
  -h, --help          this text

Environment: EXECSCOPE_VERSION, EXECSCOPE_PREFIX, EXECSCOPE_TARGET.
EOF
}

die() {
  printf 'install.sh: %s\n' "$*" >&2
  exit 1
}

while [ $# -gt 0 ]; do
  case "$1" in
    --version)
      [ $# -ge 2 ] || die "--version needs a value"
      VERSION="$2"
      shift 2
      ;;
    --prefix)
      [ $# -ge 2 ] || die "--prefix needs a value"
      PREFIX="$2"
      shift 2
      ;;
    --target)
      [ $# -ge 2 ] || die "--target needs a value"
      TARGET="$2"
      shift 2
      ;;
    --dry-run)
      DRY_RUN=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      usage >&2
      die "unknown option: $1"
      ;;
  esac
done

VERSION="${VERSION#v}"

if [ -z "$TARGET" ]; then
  os="$(uname -s)"
  arch="$(uname -m)"
  case "$os:$arch" in
    Darwin:arm64) TARGET="aarch64-apple-darwin" ;;
    Darwin:x86_64) TARGET="x86_64-apple-darwin" ;;
    Linux:x86_64|Linux:amd64) TARGET="x86_64-unknown-linux-gnu" ;;
    Linux:aarch64|Linux:arm64) TARGET="aarch64-unknown-linux-gnu" ;;
    *)
      die "no prebuilt binary for $os/$arch; set --target or build from source"
      ;;
  esac
fi

curl_ok="curl -fsSL --retry 3 --retry-delay 2"
command -v curl >/dev/null 2>&1 || die "curl is required"

if [ -z "$VERSION" ]; then
  latest_url="$($curl_ok -o /dev/null -w '%{url_effective}' "https://github.com/$REPO/releases/latest")" ||
    die "cannot reach GitHub to find the latest release"
  VERSION="${latest_url##*/}"
  VERSION="${VERSION#v}"
  case "$VERSION" in
    ""|latest) die "no published release found; pass --version with an existing tag" ;;
  esac
fi

ARCHIVE="execscope-$TARGET.tar.gz"
BASE="${EXECSCOPE_BASE_URL:-https://github.com/$REPO/releases/download/v$VERSION}/$ARCHIVE"

if [ "$DRY_RUN" -eq 1 ]; then
  printf 'would install ExecScope %s (%s)\n  from %s\n  to   %s/execscope\n' \
    "$VERSION" "$TARGET" "$BASE" "$PREFIX"
  exit 0
fi

if [ -x "$PREFIX/execscope" ]; then
  installed="$("$PREFIX/execscope" --version 2>/dev/null || true)"
  if [ "$installed" = "execscope $VERSION" ]; then
    printf 'ExecScope %s is already installed at %s/execscope\n' "$VERSION" "$PREFIX"
    exit 0
  fi
fi

tmp="$(mktemp -d "${TMPDIR:-/tmp}/execscope-install.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT INT TERM

printf 'Downloading ExecScope %s (%s)\n' "$VERSION" "$TARGET"
$curl_ok -o "$tmp/$ARCHIVE" "$BASE" ||
  die "download failed: $BASE (does tag v$VERSION have a $TARGET build?)"
$curl_ok -o "$tmp/$ARCHIVE.sha256" "$BASE.sha256" ||
  die "checksum download failed: $BASE.sha256"

(
  cd "$tmp"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum --check --strict "$ARCHIVE.sha256" >/dev/null
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 -c "$ARCHIVE.sha256" >/dev/null
  else
    die "need sha256sum or shasum to verify the download"
  fi
) || die "checksum mismatch for $ARCHIVE; the download was left unused"

tar xzf "$tmp/$ARCHIVE" -C "$tmp" execscope ||
  die "cannot unpack $ARCHIVE"
[ -f "$tmp/execscope" ] || die "$ARCHIVE does not contain an execscope binary"

mkdir -p "$PREFIX"
install -m 755 "$tmp/execscope" "$PREFIX/execscope"

case "$(uname -s)" in
  Darwin) xattr -d com.apple.quarantine "$PREFIX/execscope" 2>/dev/null || true ;;
esac

installed="$("$PREFIX/execscope" --version)" || die "$PREFIX/execscope does not run on this machine"
printf 'Installed %s at %s/execscope\n' "$installed" "$PREFIX"

case ":$PATH:" in
  *":$PREFIX:"*) ;;
  *) printf 'note: %s is not on PATH; add it to use execscope by name\n' "$PREFIX" ;;
esac
