#!/usr/bin/env bash
#
# Build and install the Rust `cloner` binary.
#
# Usage:
#   ./install.sh                 # install into ~/.local/bin
#   ./install.sh --system        # install into /usr/local/bin (uses sudo)
#   ./install.sh --prefix DIR    # install into DIR/bin
#   ./install.sh --no-deps       # skip OS package installation
#   ./install.sh --no-build      # install an already built binary
#   ./install.sh --dry-run       # print what would happen
#
set -Eeuo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$SCRIPT_DIR"
PREFIX="${HOME}/.local"
SYSTEM_INSTALL=0
INSTALL_DEPS=1
BUILD_BINARY=1
INSTALL_BINARY=1
DRY_RUN=0
TOOLCHAIN="${CLONER_RUST_TOOLCHAIN:-stable}"

log() { printf '\033[1;34m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33mwarning:\033[0m %s\n' "$*" >&2; }
die() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

usage() {
  cat <<'EOF'
Build and install the Rust cloner binary.

Usage: ./install.sh [OPTIONS]

Options:
  --prefix DIR     Install the binary into DIR/bin (default: ~/.local/bin)
  --system         Install into /usr/local/bin using sudo
  --no-deps        Do not install OS build dependencies
  --no-build       Do not run cargo build; install target/release/cloner
  --no-install     Build only; do not copy the binary anywhere
  --dry-run        Print commands instead of running them
  -h, --help       Show this help

Environment:
  CLONER_RUST_TOOLCHAIN   Rust toolchain for rustup (default: stable)

Examples:
  ./install.sh
  ./install.sh --system
  ./install.sh --prefix "$HOME/opt/cloner" --no-deps
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --prefix)
      [[ $# -ge 2 ]] || die "--prefix requires a directory"
      PREFIX="$2"
      shift 2
      ;;
    --system)
      PREFIX="/usr/local"
      SYSTEM_INSTALL=1
      shift
      ;;
    --no-deps)
      INSTALL_DEPS=0
      shift
      ;;
    --no-build)
      BUILD_BINARY=0
      shift
      ;;
    --no-install)
      INSTALL_BINARY=0
      shift
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
      die "unknown option: $1 (use --help)"
      ;;
  esac
done

run() {
  if [[ "$DRY_RUN" -eq 1 ]]; then
    printf '  [dry-run] %s\n' "$*"
  else
    "$@"
  fi
}

run_in() {
  local dir="$1"
  shift
  if [[ "$DRY_RUN" -eq 1 ]]; then
    printf '  [dry-run] (cd %s && %s)\n' "$dir" "$*"
  else
    (cd "$dir" && "$@")
  fi
}

run_privileged() {
  if [[ "$(id -u)" -eq 0 ]]; then
    run "$@"
  elif command -v sudo >/dev/null 2>&1; then
    run sudo "$@"
  else
    die "root privileges are required, but sudo is not installed"
  fi
}

install_linux_deps() {
  if command -v apt-get >/dev/null 2>&1; then
    run_privileged apt-get update
    run_privileged env DEBIAN_FRONTEND=noninteractive apt-get install -y \
      build-essential curl git ca-certificates pkg-config
  elif command -v dnf >/dev/null 2>&1; then
    run_privileged dnf install -y \
      gcc gcc-c++ make curl git ca-certificates pkgconf-pkg-config
  elif command -v yum >/dev/null 2>&1; then
    run_privileged yum install -y \
      gcc gcc-c++ make curl git ca-certificates pkgconfig
  elif command -v pacman >/dev/null 2>&1; then
    run_privileged pacman -Sy --needed --noconfirm \
      base-devel curl git ca-certificates pkgconf
  elif command -v zypper >/dev/null 2>&1; then
    run_privileged zypper --non-interactive install \
      gcc gcc-c++ make curl git ca-certificates pkg-config
  elif command -v apk >/dev/null 2>&1; then
    run_privileged apk add --no-cache \
      build-base curl git ca-certificates pkgconf
  else
    warn "unknown Linux package manager; install a C toolchain, make, git and curl manually"
    for tool in cc make git curl; do
      command -v "$tool" >/dev/null 2>&1 || die "missing required build tool: $tool"
    done
  fi
}

install_macos_deps() {
  if ! xcode-select -p >/dev/null 2>&1; then
    log "Xcode Command Line Tools are required"
    if [[ "$DRY_RUN" -eq 1 ]]; then
      printf '  [dry-run] xcode-select --install\n'
    else
      xcode-select --install || true
      die "finish the Xcode Command Line Tools installation, then re-run this script"
    fi
  fi
  for tool in cc make git curl; do
    command -v "$tool" >/dev/null 2>&1 || die "missing required build tool: $tool"
  done
}

install_deps() {
  if [[ "$INSTALL_DEPS" -eq 0 ]]; then
    log "skipping OS dependency installation (--no-deps)"
    return
  fi

  log "installing build dependencies"
  case "$(uname -s)" in
    Linux) install_linux_deps ;;
    Darwin) install_macos_deps ;;
    *)
      warn "unsupported OS: $(uname -s); expecting an existing Rust/C toolchain"
      ;;
  esac
}

install_rust() {
  if command -v cargo >/dev/null 2>&1; then
    log "using existing cargo: $(command -v cargo)"
  else
    log "installing Rust via rustup ($TOOLCHAIN)"
    if [[ "$DRY_RUN" -eq 1 ]]; then
      printf '  [dry-run] curl https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain %s\n' "$TOOLCHAIN"
    else
      curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
        | sh -s -- -y --profile minimal --default-toolchain "$TOOLCHAIN"
    fi
    if [[ -f "${HOME}/.cargo/env" ]]; then
      # shellcheck disable=SC1091
      . "${HOME}/.cargo/env"
    fi
    export PATH="${HOME}/.cargo/bin:${PATH}"
  fi

  if [[ "$DRY_RUN" -eq 0 ]] && ! command -v cargo >/dev/null 2>&1; then
    die "cargo was not found after installing Rust; open a new shell and retry"
  fi
}

build_cloner() {
  if [[ "$BUILD_BINARY" -eq 0 ]]; then
    log "skipping build (--no-build)"
    return
  fi
  [[ -f "${PROJECT_DIR}/Cargo.toml" ]] || die "Cargo.toml not found in ${PROJECT_DIR}"
  log "building cloner (release)"
  run_in "$PROJECT_DIR" cargo build --release --locked
}

install_binary() {
  if [[ "$INSTALL_BINARY" -eq 0 ]]; then
    log "skipping binary installation (--no-install)"
    return
  fi

  local source="${PROJECT_DIR}/target/release/cloner"
  local destination="${PREFIX}/bin/cloner"

  if [[ "$DRY_RUN" -eq 0 && ! -x "$source" ]]; then
    die "built binary not found at ${source}; run without --no-build"
  fi

  log "installing to ${destination}"
  if [[ "$SYSTEM_INSTALL" -eq 1 && "$(id -u)" -ne 0 ]]; then
    run_privileged install -d "${PREFIX}/bin"
    run_privileged install -m 0755 "$source" "$destination"
  else
    run install -d "${PREFIX}/bin"
    run install -m 0755 "$source" "$destination"
  fi

  case ":${PATH}:" in
    *":${PREFIX}/bin:"*) ;;
    *)
      warn "${PREFIX}/bin is not in PATH"
      printf "  Add this to your shell profile:\n\n    export PATH=\"%s/bin:\$PATH\"\n\n" "$PREFIX"
      ;;
  esac

  if [[ "$DRY_RUN" -eq 0 ]]; then
    "${destination}" --version
  fi
}

main() {
  log "cloner installer"
  install_deps
  install_rust
  build_cloner
  install_binary

  if [[ "$DRY_RUN" -eq 1 ]]; then
    log "dry run complete"
    return
  fi

  log "done"
  if [[ "$INSTALL_BINARY" -eq 1 ]]; then
    if command -v cloner >/dev/null 2>&1; then
      printf '\nRun:\n\n  cloner --help\n  cloner validate\n  cloner run\n\n'
    else
      printf '\nRun:\n\n  %s/bin/cloner --help\n\n' "$PREFIX"
    fi
  fi
}

main "$@"
