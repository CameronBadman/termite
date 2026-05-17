#!/usr/bin/env bash
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
samply="${SAMPLY:-$(command -v samply || printf '%s/.cargo/bin/samply' "$HOME")}"
payload_kind="${PAYLOAD:-huge}"
lines="${LINES:-500000}"
timeout_s="${TIMEOUT:-120s}"
rate="${RATE:-1000}"
stamp="$(date +%Y%m%d-%H%M%S)"
out_dir="${OUT_DIR:-$repo/target/profiles}"
tmpdir="$(mktemp -d)"

cleanup() {
    rm -rf "$tmpdir"
}
trap cleanup EXIT

if [[ ! -x "$samply" ]]; then
    printf 'samply not found at %s\n' "$samply" >&2
    printf 'install with: cargo install samply\n' >&2
    exit 1
fi

mkdir -p "$out_dir"

payload="$tmpdir/payload.txt"
child="$tmpdir/profile-child"
runner="$tmpdir/profile-runner"
profile="$out_dir/termite-${payload_kind}-${lines}-${stamp}.json.gz"
log="$out_dir/termite-${payload_kind}-${lines}-${stamp}.log"

case "$payload_kind" in
    ansi)
        awk -v lines="$lines" 'BEGIN {
            for (i = 0; i < lines; i++) {
                color = 31 + (i % 7)
                printf "\033[%dmansi line %06d abcdefghijklmnopqrstuvwxyz 0123456789 []{}<> ~!@#$%%^&*()\033[0m\r\n", color, i
            }
        }' > "$payload"
        ;;
    unicode)
        awk -v lines="$lines" 'BEGIN {
            for (i = 0; i < lines; i++) {
                printf "unicode line %06d 表 λ π ┌─┐ █ ░ we’ll — ok  \r\n", i
            }
        }' > "$payload"
        ;;
    huge)
        awk -v lines="$lines" 'BEGIN {
            for (i = 0; i < lines; i++) {
                printf "huge line %06d abcdefghijklmnopqrstuvwxyz 0123456789\r\n", i
            }
        }' > "$payload"
        ;;
    *)
        printf 'unknown PAYLOAD: %s\n' "$payload_kind" >&2
        exit 1
        ;;
esac

{
    printf '#!/usr/bin/env bash\n'
    printf 'set -euo pipefail\n'
    printf 'cat %q\n' "$payload"
} > "$child"
chmod +x "$child"

{
    printf '#!/usr/bin/env bash\n'
    printf 'set -euo pipefail\n'
    printf 'TERMITE_PERF=1 timeout %q env SHELL=%q %q >%q 2>&1\n' \
        "$timeout_s" "$child" "$repo/target/release/termite" "$log"
} > "$runner"
chmod +x "$runner"

printf 'building release terminal with debug symbols...\n'
CARGO_PROFILE_RELEASE_DEBUG=1 cargo build --release -p termite >/dev/null

printf 'payload=%s lines=%s bytes=%s\n' "$payload_kind" "$lines" "$(wc -c < "$payload")"
printf 'profile=%s\n' "$profile"
printf 'log=%s\n' "$log"

start="$(date +%s%N)"
status=ok
if ! "$samply" record \
    --save-only \
    --rate "$rate" \
    --profile-name "termite-${payload_kind}-${lines}" \
    --output "$profile" \
    -- "$runner"; then
    status=fail
fi
elapsed_ns=$(($(date +%s%N) - start))
awk -v ns="$elapsed_ns" -v status="$status" 'BEGIN {
    printf "elapsed=%.2fms status=%s\n", ns / 1000000, status
}'

printf '\n== termite-perf samples ==\n'
if ! rg 'termite-profile|termite-perf' "$log"; then
    printf 'no TERMITE_PERF samples captured; inspect %s\n' "$log"
fi
