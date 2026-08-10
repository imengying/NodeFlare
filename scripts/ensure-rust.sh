#!/bin/sh
set -eu

wasm_target_ready() {
  command -v rustc >/dev/null 2>&1 || return 1
  target_libdir=$(rustc --print target-libdir --target wasm32-unknown-unknown 2>/dev/null) || return 1
  test -d "$target_libdir"
}

if ! command -v cargo >/dev/null 2>&1 || ! wasm_target_ready; then
  if ! command -v rustup >/dev/null 2>&1; then
    rustup_install_url="https://sh.rustup.rs"
    rustup_installer=$(mktemp)
    trap 'rm -f "$rustup_installer"' 0 HUP INT TERM
    echo "Rust toolchain not found; installing stable Rust..."
    if command -v curl >/dev/null 2>&1; then
      curl --proto '=https' --tlsv1.2 -sSf "$rustup_install_url" -o "$rustup_installer"
    elif command -v wget >/dev/null 2>&1; then
      wget -qO "$rustup_installer" "$rustup_install_url"
    else
      echo "Rust is missing and curl/wget is unavailable" >&2
      exit 1
    fi
    sh "$rustup_installer" -y --profile minimal --default-toolchain stable
    rm -f "$rustup_installer"
    trap - 0 HUP INT TERM
  fi

  rust_bin_dir="${CARGO_HOME:-$HOME/.cargo}/bin"
  export PATH="$rust_bin_dir:$PATH"
  if ! command -v cargo >/dev/null 2>&1; then
    rustup toolchain install stable --profile minimal
    rustup default stable
  fi
  rustup target add wasm32-unknown-unknown
fi

if ! command -v cargo >/dev/null 2>&1 || ! wasm_target_ready; then
  echo "Rust Cargo or wasm32-unknown-unknown target is unavailable" >&2
  exit 1
fi
