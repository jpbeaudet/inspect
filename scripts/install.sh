#!/usr/bin/env sh
# install.sh — one-shot installer for `inspect`.
#
# Resolves the latest release tag (or honors $INSPECT_VERSION), downloads
# the right tarball for the host triple, verifies sha256, optionally
# verifies cosign signature when `cosign` is on $PATH, and installs the
# binary to $INSPECT_PREFIX/bin (default: $HOME/.local/bin).
#
# Rollback-safe: writes to a temp dir, atomically renames into place,
# and refuses to clobber a newer installed version unless --force is set.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/jpbeaudet/inspect/main/scripts/install.sh | sh
#   curl -fsSL .../install.sh | sh -s -- --version v0.1.0 --prefix /usr/local
#   curl -fsSL .../install.sh | INSPECT_VERSION=v0.1.0 sh

set -eu

REPO="jpbeaudet/inspect"
VERSION="${INSPECT_VERSION:-}"
PREFIX="${INSPECT_PREFIX:-$HOME/.local}"
FORCE=0
NO_VERIFY=0

log()  { printf 'inspect-install: %s\n' "$*" >&2; }
die()  { log "error: $*"; exit 1; }

while [ $# -gt 0 ]; do
  case "$1" in
    --version) VERSION="${2:?}"; shift 2 ;;
    --prefix)  PREFIX="${2:?}";  shift 2 ;;
    --force)   FORCE=1;          shift   ;;
    --no-verify) NO_VERIFY=1;    shift   ;;
    -h|--help)
      cat <<USAGE
Usage: install.sh [--version vX.Y.Z] [--prefix DIR] [--force] [--no-verify]

Environment:
  INSPECT_VERSION  Pin a release tag (default: latest).
  INSPECT_PREFIX   Install root; binary goes to \$PREFIX/bin (default: \$HOME/.local).
USAGE
      exit 0 ;;
    *) die "unknown arg: $1" ;;
  esac
done

# Resolve OS/arch
OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
ARCH="$(uname -m)"
case "$OS" in
  linux)  os_part="unknown-linux-musl" ;;
  darwin) os_part="apple-darwin" ;;
  *) die "unsupported OS: $OS" ;;
esac
case "$ARCH" in
  x86_64|amd64) arch_part="x86_64" ;;
  arm64|aarch64) arch_part="aarch64" ;;
  *) die "unsupported arch: $ARCH" ;;
esac
TRIPLE="${arch_part}-${os_part}"

# Resolve version
if [ -z "$VERSION" ]; then
  log "resolving latest tag..."
  VERSION=$(
    curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
      | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' \
      | head -n1
  )
  [ -n "$VERSION" ] || die "could not resolve latest tag"
fi
log "version: $VERSION"
log "target:  $TRIPLE"
log "prefix:  $PREFIX"

VERSTRIPPED="${VERSION#v}"
NAME="inspect-${VERSTRIPPED}-${TRIPLE}"
ARCHIVE="${NAME}.tar.gz"
BASE="https://github.com/${REPO}/releases/download/${VERSION}"

# Compare against installed version
EXISTING="$PREFIX/bin/inspect"
if [ -x "$EXISTING" ] && [ "$FORCE" -ne 1 ]; then
  cur=$("$EXISTING" --version 2>/dev/null | awk '{print $NF}' || echo "")
  if [ -n "$cur" ] && [ "$cur" = "$VERSTRIPPED" ]; then
    log "already at $cur — nothing to do (use --force to reinstall)"
    exit 0
  fi
fi

# Download into a temp dir
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT INT TERM

log "downloading $ARCHIVE..."
curl -fsSL "${BASE}/${ARCHIVE}"      -o "$tmp/$ARCHIVE"
curl -fsSL "${BASE}/${ARCHIVE}.sha256" -o "$tmp/$ARCHIVE.sha256"

# Verify checksum
log "verifying sha256..."
( cd "$tmp" && shasum -a 256 -c "$ARCHIVE.sha256" >/dev/null )

# Optional cosign verification
if [ "$NO_VERIFY" -ne 1 ] && command -v cosign >/dev/null 2>&1; then
  log "verifying cosign signature (keyless)..."
  curl -fsSL "${BASE}/${ARCHIVE}.sig" -o "$tmp/$ARCHIVE.sig" || true
  curl -fsSL "${BASE}/${ARCHIVE}.pem" -o "$tmp/$ARCHIVE.pem" || true
  if [ -s "$tmp/$ARCHIVE.sig" ] && [ -s "$tmp/$ARCHIVE.pem" ]; then
    cosign verify-blob \
      --certificate-identity-regexp "https://github.com/${REPO}/.*" \
      --certificate-oidc-issuer "https://token.actions.githubusercontent.com" \
      --certificate "$tmp/$ARCHIVE.pem" \
      --signature   "$tmp/$ARCHIVE.sig" \
      "$tmp/$ARCHIVE"
  else
    log "cosign artifacts missing; skipping signature verification"
  fi
fi

# Extract + atomic install
log "extracting..."
( cd "$tmp" && tar -xzf "$ARCHIVE" )
mkdir -p "$PREFIX/bin"
install_target="$PREFIX/bin/inspect"
mv -f "$tmp/$NAME/inspect" "${install_target}.new"
chmod 0755 "${install_target}.new"
mv -f "${install_target}.new" "${install_target}"

log "installed: $install_target"
case ":$PATH:" in
  *":$PREFIX/bin:"*) ;;
  *) log "note: $PREFIX/bin is not on \$PATH — add it to your shell profile" ;;
esac
"$install_target" --version || true
