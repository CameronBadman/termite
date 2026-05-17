#!/usr/bin/env bash
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
payload_kind="${PAYLOAD:-huge}"
lines="${LINES:-500000}"
timeout_s="${TIMEOUT:-120s}"
hz="${HZ:-997}"
stamp="$(date +%Y%m%d-%H%M%S)"
out_dir="${OUT_DIR:-$repo/target/profiles}"
tmpdir="$(mktemp -d)"

cleanup() {
    rm -rf "$tmpdir"
}
trap cleanup EXIT

mkdir -p "$out_dir"

payload="$tmpdir/payload.txt"
child="$tmpdir/profile-child"
profile="$out_dir/termite-pprof-${payload_kind}-${lines}-${stamp}.svg"
log="$out_dir/termite-pprof-${payload_kind}-${lines}-${stamp}.log"

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

printf 'building release terminal with pprof support...\n'
CARGO_PROFILE_RELEASE_DEBUG=1 cargo build --release -p termite --features profile >/dev/null

printf 'payload=%s lines=%s bytes=%s\n' "$payload_kind" "$lines" "$(wc -c < "$payload")"
printf 'profile=%s\n' "$profile"
printf 'log=%s\n' "$log"

start="$(date +%s%N)"
status=ok
if ! TERMITE_PERF=1 TERMITE_PPROF="$profile" TERMITE_PPROF_HZ="$hz" \
    timeout "$timeout_s" env SHELL="$child" "$repo/target/release/termite" >"$log" 2>&1; then
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

if [[ -s "$profile" ]]; then
    printf '\n== hottest flamegraph frames ==\n'
    mapfile -t hottest_frames < <(
        perl -0ne 'while (/<title>(.*?)<\/title>/g) { print "$1\n" }' "$profile" \
            | sort -t '(' -k2,2nr
    )
    printf '%s\n' "${hottest_frames[@]:0:20}"

    printf '\n== hottest termite frames ==\n'
    mapfile -t hottest_termite_frames < <(
        perl -0ne 'while (/<title>(.*?)<\/title>/g) { print "$1\n" }' "$profile" \
            | rg 'termite(_core)?::' \
            | rg -v 'termite::main|profiler::run|runner::Runner::run' \
            | sort -t '(' -k2,2nr
    )
    printf '%s\n' "${hottest_termite_frames[@]:0:20}"
fi
