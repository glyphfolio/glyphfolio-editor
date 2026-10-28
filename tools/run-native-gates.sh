#!/usr/bin/env bash
# Opt-in, visible macOS tests against the disposable editor only.
set -uo pipefail

if [[ "${1:-}" != "--allow-temporary-setup" || "$(uname -s)" != Darwin ]]; then
    printf '%s\n' 'Run on macOS after allowing temporary native input/VoiceOver setup:' \
        'bash tools/run-native-gates.sh --allow-temporary-setup [--with-voiceover]' \
        'Leave the editor focused; do not copy or use other applications during the batch.'
    exit 2
fi

native_project="$(cd -- "$(dirname -- "$0")/.." && pwd)"
native_demo="$native_project/target/GlyphFolioEditor.app/Contents/MacOS/glyphfolio-editor-demo"
native_keys="$native_project/target/post-native-keys"
native_clipboard="$native_project/target/clipboard-ui-probe"
native_evidence="$native_project/docs/evidence"
native_cache="$native_project/target/swift-module-cache"
native_pid=''
native_failed=0
native_voiceover=no
if [[ "${2:-}" == "--with-voiceover" ]]; then
    native_voiceover=yes
elif [[ -n "${2:-}" ]]; then
    printf '%s\n' 'Optional second argument: --with-voiceover (experimental, supervised only).' >&2
    exit 2
fi

for native_required in "$native_demo" "$native_keys" "$native_clipboard"; do
    if [[ ! -x "$native_required" ]]; then
        printf 'Missing prepared native test executable: %s\n' "$native_required" >&2
        exit 2
    fi
done

native_close_demo() {
    if [[ -n "$native_pid" ]]; then
        kill -TERM "$native_pid" 2>/dev/null || true
        wait "$native_pid" 2>/dev/null || true
        native_pid=''
    fi
}
trap native_close_demo EXIT
trap 'native_close_demo; exit 130' INT TERM

printf '%s\n' 'NATIVE_BATCH clipboard and editable accessibility'
"$native_demo" --native-test-ui > "$native_evidence/platform-session.log" 2>&1 &
native_pid=$!
sleep 2
"$native_clipboard" --pid "$native_pid" --output "$native_evidence/platform-clipboard.json" || native_failed=1
if [[ "$native_voiceover" == yes ]]; then
    /usr/bin/swift -module-cache-path "$native_cache" "$native_project/tools/accessibility_probe.swift" \
        --pid "$native_pid" --output "$native_evidence/platform-ax.json" --with-voiceover || native_failed=1
else
    /usr/bin/swift -module-cache-path "$native_cache" "$native_project/tools/accessibility_probe.swift" \
        --pid "$native_pid" --output "$native_evidence/platform-ax.json" || native_failed=1
fi
native_close_demo

printf '%s\n' 'NATIVE_BATCH Japanese cancellation'
/usr/bin/swift -module-cache-path "$native_cache" "$native_project/tools/ime_probe.swift" \
    --source com.apple.inputmethod.Kotoeri.RomajiTyping.Japanese --enable-temporarily \
    --binary "$native_demo" --key-driver "$native_keys" --key-mode cancel --key-transport hid \
    --timeout 30 -- --native-ime-observe cancel | tee "$native_evidence/platform-ime-cancel.log" || native_failed=1

printf '%s\n' 'NATIVE_BATCH Chinese composition'
/usr/bin/swift -module-cache-path "$native_cache" "$native_project/tools/ime_probe.swift" \
    --source com.apple.inputmethod.SCIM.ITABC --enable-temporarily \
    --binary "$native_demo" --key-driver "$native_keys" --key-mode chinese --key-transport hid \
    --candidate-output "$native_evidence/platform-chinese-candidate.json" \
    --timeout 30 -- --native-ime-observe chinese | tee "$native_evidence/platform-chinese.log" || native_failed=1

printf '%s\n' 'NATIVE_BATCH Japanese viewport and host barriers'
/usr/bin/swift -module-cache-path "$native_cache" "$native_project/tools/ime_probe.swift" \
    --source com.apple.inputmethod.Kotoeri.RomajiTyping.Japanese --enable-temporarily \
    --binary "$native_demo" --key-driver "$native_keys" --key-mode japanese --key-transport hid \
    --key-exercise-viewport yes --candidate-output "$native_evidence/platform-ime-viewport-candidate.json" \
    --timeout 30 -- --native-ime-observe japanese --native-ime-viewport | tee "$native_evidence/platform-ime-viewport.log" || native_failed=1

printf '%s\n' 'NATIVE_BATCH Japanese native focus transfer'
/usr/bin/swift -module-cache-path "$native_cache" "$native_project/tools/ime_probe.swift" \
    --source com.apple.inputmethod.Kotoeri.RomajiTyping.Japanese --enable-temporarily \
    --binary "$native_demo" --key-driver "$native_keys" --key-mode focus --key-transport hid \
    --timeout 30 -- --native-ime-observe focus | tee "$native_evidence/platform-ime-focus.log" || native_failed=1

printf 'NATIVE_BATCH complete failures=%s; inspect each gate and restoration report\n' "$native_failed"
if [[ "$native_voiceover" == no ]]; then
    printf '%s\n' 'VoiceOver interaction NOT_RUN; this batch cannot qualify F4 by itself.'
fi
exit "$native_failed"
