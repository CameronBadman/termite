#!/usr/bin/env bash
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
payload_kind="${PAYLOAD:-huge}"
lines="${LINES:-500000}"
timeout_s="${TIMEOUT:-120s}"
tmpdir="$(mktemp -d)"
log="${LOG:-/tmp/termite-profile-$(date +%s).log}"

cleanup() {
    rm -rf "$tmpdir"
}
trap cleanup EXIT

payload="$tmpdir/payload.txt"
child="$tmpdir/profile-child"

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

printf 'building release terminal...\n'
cargo build --release -p termite >/dev/null

printf 'payload=%s lines=%s bytes=%s\n' "$payload_kind" "$lines" "$(wc -c < "$payload")"
printf 'log=%s\n' "$log"

start="$(date +%s%N)"
status=ok
if ! TERMITE_PERF=1 timeout "$timeout_s" env SHELL="$child" "$repo/target/release/termite" >"$log" 2>&1; then
    status=fail
fi
elapsed_ns=$(($(date +%s%N) - start))
awk -v ns="$elapsed_ns" -v status="$status" 'BEGIN {
    printf "elapsed=%.2fms status=%s\n", ns / 1000000, status
}'

printf '\n== termite-perf samples ==\n'
if ! rg 'termite-perf' "$log"; then
    printf 'no TERMITE_PERF samples captured; increase LINES or inspect %s\n' "$log"
fi
