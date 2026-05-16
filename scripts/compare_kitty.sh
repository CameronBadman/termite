#!/usr/bin/env bash
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
runs="${RUNS:-5}"
lines="${LINES:-50000}"
timeout_s="${TIMEOUT:-60s}"
tmpdir="$(mktemp -d)"
payload="$tmpdir/payload.txt"
bench_shell="$tmpdir/bench-shell"

cleanup() {
    rm -rf "$tmpdir"
}
trap cleanup EXIT

awk -v lines="$lines" 'BEGIN {
    for (i = 0; i < lines; i++) {
        color = 31 + (i % 7)
        printf "\033[%dmbench line %06d abcdefghijklmnopqrstuvwxyz 0123456789 []{}<> ~!@#$%%^&*()\033[0m\r\n", color, i
    }
}' > "$payload"

{
    printf '#!/usr/bin/env bash\n'
    printf 'cat %q\n' "$payload"
} > "$bench_shell"
chmod +x "$bench_shell"

payload_bytes="$(wc -c < "$payload")"
printf 'payload: %s lines, %s bytes\n' "$lines" "$payload_bytes"

cargo build --release -p termite >/dev/null

measure() {
    local label="$1"
    shift
    local total_ns=0
    local ok=0

    printf '\n%s\n' "$label"
    for i in $(seq 1 "$runs"); do
        local log="$tmpdir/${label// /_}-$i.log"
        local start end elapsed_ns elapsed_ms status
        start="$(date +%s%N)"
        if timeout "$timeout_s" "$@" >"$log" 2>&1; then
            status="ok"
            ok=$((ok + 1))
        else
            status="fail"
        fi
        end="$(date +%s%N)"
        elapsed_ns=$((end - start))
        elapsed_ms="$(awk -v ns="$elapsed_ns" 'BEGIN { printf "%.2f", ns / 1000000 }')"
        printf '  run %d: %sms %s\n' "$i" "$elapsed_ms" "$status"
        if [[ "$status" == "ok" ]]; then
            total_ns=$((total_ns + elapsed_ns))
        else
            sed -n '1,20p' "$log"
        fi
    done

    if ((ok > 0)); then
        awk -v ns="$total_ns" -v ok="$ok" -v bytes="$payload_bytes" \
            'BEGIN {
                avg = ns / ok
                seconds = avg / 1000000000
                mib = bytes / 1048576
                printf "  avg: %.2fms, %.2f MiB/s\n", avg / 1000000, mib / seconds
            }'
    fi
}

measure "termite" env SHELL="$bench_shell" "$repo/target/release/termite"
measure "kitty" kitty --config NONE --class termite-kitty-bench --title termite-kitty-bench "$bench_shell"
