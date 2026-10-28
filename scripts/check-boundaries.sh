#!/usr/bin/env bash
set -euo pipefail

boundary_target="$(rustc -vV | sed -n 's/^host: //p' | tr -d '\r')"
if test -z "$boundary_target"; then
    printf '%s\n' 'Cannot determine native target.' >&2
    exit 1
fi

for boundary_crate in glyphfolio-editor-core glyphfolio-editor-markdown glyphfolio-editor-layout; do
    boundary_tree="$(cargo tree --locked --target "$boundary_target" --edges normal --prefix none --format '{p}' -p "$boundary_crate")"
    if printf '%s\n' "$boundary_tree" | grep -Eiq '^(egui|eframe|tauri|wry|webkit|glyphfolio-migration|glyphfolio-plugin|glyphfolio-services)([- ]|$)'; then
        printf 'Forbidden application/UI dependency in %s\n' "$boundary_crate" >&2
        exit 1
    fi
done

boundary_tree="$(cargo tree --locked --workspace --target "$boundary_target" --edges normal --prefix none --format '{p}')"
if printf '%s\n' "$boundary_tree" | grep -Eiq '^(egui[_-]richedit|egui[_-]pop[_-]editors|ferrite-editor|leaf-editor|gpui-editor)([- ]|$)'; then
    printf '%s\n' 'Forbidden existing editor dependency.' >&2
    exit 1
fi
printf 'PASS: native dependency boundaries (%s). API/provenance review is also required.\n' "$boundary_target"
