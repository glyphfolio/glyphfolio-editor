#!/usr/bin/env bash
set -euo pipefail

cd -- "$(dirname -- "$0")/.."
if ! command -v cargo >/dev/null 2>&1; then
    for editor_cargo in "$HOME"/.rustup/toolchains/stable-*/bin/cargo; do
        if test -x "$editor_cargo"; then
            export PATH="${editor_cargo%/cargo}:$PATH"
            break
        fi
    done
fi
if ! command -v cargo >/dev/null 2>&1; then
    printf '%s\n' 'Install Rust 1.99.1 or newer and put cargo on PATH.' >&2
    exit 1
fi
exec cargo run --locked -p glyphfolio-editor-demo -- "$@"
