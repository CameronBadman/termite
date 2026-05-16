#!/usr/bin/env bash
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
runs="${RUNS:-5}"
lines="${LINES:-30000}"
huge_lines="${HUGE_LINES:-120000}"
timeout_s="${TIMEOUT:-90s}"
tmpdir="$(mktemp -d)"

cleanup() {
    rm -rf "$tmpdir"
}
trap cleanup EXIT

make_payload() {
    local name="$1"
    local count="$2"
    local file="$tmpdir/$name.txt"
    case "$name" in
        ansi)
            awk -v lines="$count" 'BEGIN {
                for (i = 0; i < lines; i++) {
                    color = 31 + (i % 7)
                    printf "\033[%dmansi line %06d abcdefghijklmnopqrstuvwxyz 0123456789 []{}<> ~!@#$%%^&*()\033[0m\r\n", color, i
                }
            }' > "$file"
            ;;
        unicode)
            awk -v lines="$count" 'BEGIN {
                for (i = 0; i < lines; i++) {
                    printf "unicode line %06d 表 λ π ┌─┐ █ ░ we’ll — ok  \r\n", i
                }
            }' > "$file"
            ;;
        huge)
            awk -v lines="$count" 'BEGIN {
                for (i = 0; i < lines; i++) {
                    printf "huge line %06d abcdefghijklmnopqrstuvwxyz 0123456789\r\n", i
                }
            }' > "$file"
            ;;
        *)
            printf 'unknown payload: %s\n' "$name" >&2
            return 1
            ;;
    esac
    printf '%s' "$file"
}

make_live_child() {
    local name="$1"
    local payload="$2"
    local ready="$3"
    local trigger="$4"
    local file="$tmpdir/$name-child"
    {
        printf '#!/usr/bin/env bash\n'
        printf 'set -euo pipefail\n'
        printf ': > %q\n' "$ready"
        printf 'while [ ! -e %q ]; do sleep 0.001; done\n' "$trigger"
        printf 'cat %q\n' "$payload"
    } > "$file"
    chmod +x "$file"
    printf '%s' "$file"
}

make_startup_child() {
    local name="$1"
    local ready="$2"
    local file="$tmpdir/$name-child"
    {
        printf '#!/usr/bin/env bash\n'
        printf 'set -euo pipefail\n'
        printf ': > %q\n' "$ready"
    } > "$file"
    chmod +x "$file"
    printf '%s' "$file"
}

wait_ready() {
    local ready="$1"
    local pid="$2"
    local deadline=$((SECONDS + 15))
    while [ ! -e "$ready" ]; do
        if ! kill -0 "$pid" 2>/dev/null; then
            return 1
        fi
        if ((SECONDS >= deadline)); then
            return 1
        fi
        sleep 0.002
    done
}

wait_for_exit() {
    local pid="$1"
    timeout "$timeout_s" tail --pid="$pid" -s 0.01 -f /dev/null >/dev/null 2>&1
}

terminal_available() {
    case "$1" in
        termite) return 0 ;;
        foot) command -v foot >/dev/null 2>&1 ;;
        kitty) command -v kitty >/dev/null 2>&1 ;;
        alacritty) command -v alacritty >/dev/null 2>&1 ;;
        *) return 1 ;;
    esac
}

launch_terminal() {
    local label="$1"
    local child="$2"
    local log="$3"
    case "$label" in
        termite)
            env SHELL="$child" "$repo/target/release/termite" >"$log" 2>&1 &
            ;;
        foot)
            foot --app-id termite-foot-live --title termite-foot-live "$child" >"$log" 2>&1 &
            ;;
        kitty)
            kitty --config NONE --class termite-kitty-live --title termite-kitty-live "$child" >"$log" 2>&1 &
            ;;
        alacritty)
            alacritty --class termite-alacritty-live --title termite-alacritty-live -e "$child" >"$log" 2>&1 &
            ;;
        *)
            return 1
            ;;
    esac
}

measure_startup() {
    local label="$1"
    local total_ready_ns=0
    local total_exit_ns=0
    local ok=0

    terminal_available "$label" || return 0
    printf '  %s\n' "$label"
    for i in $(seq 1 "$runs"); do
        local ready="$tmpdir/start-$label-$i.ready"
        local child log pid start ready_ns end exit_ns
        child="$(make_startup_child "start-$label-$i" "$ready")"
        log="$tmpdir/start-$label-$i.log"
        start="$(date +%s%N)"
        launch_terminal "$label" "$child" "$log"
        pid="$!"
        if wait_ready "$ready" "$pid"; then
            ready_ns=$(($(date +%s%N) - start))
            if wait_for_exit "$pid" && wait "$pid"; then
                end="$(date +%s%N)"
                exit_ns=$((end - start))
                ok=$((ok + 1))
                total_ready_ns=$((total_ready_ns + ready_ns))
                total_exit_ns=$((total_exit_ns + exit_ns))
                printf '    run %d: ready %.2fms exit %.2fms ok\n' \
                    "$i" \
                    "$(awk -v ns="$ready_ns" 'BEGIN { printf "%.2f", ns / 1000000 }')" \
                    "$(awk -v ns="$exit_ns" 'BEGIN { printf "%.2f", ns / 1000000 }')"
            else
                printf '    run %d: fail after ready\n' "$i"
            fi
        else
            printf '    run %d: fail before ready\n' "$i"
            sed -n '1,20p' "$log"
            kill "$pid" 2>/dev/null || true
            wait "$pid" 2>/dev/null || true
        fi
    done
    if ((ok > 0)); then
        awk -v ready="$total_ready_ns" -v exit_ns="$total_exit_ns" -v ok="$ok" 'BEGIN {
            printf "    avg ready: %.2fms, avg exit: %.2fms\n", ready / ok / 1000000, exit_ns / ok / 1000000
        }'
    fi
}

measure_live_payload() {
    local label="$1"
    local payload_name="$2"
    local payload="$3"
    local payload_bytes="$4"
    local total_ready_ns=0
    local total_live_ns=0
    local ok=0

    terminal_available "$label" || return 0
    printf '  %s\n' "$label"
    for i in $(seq 1 "$runs"); do
        local ready="$tmpdir/live-$payload_name-$label-$i.ready"
        local trigger="$tmpdir/live-$payload_name-$label-$i.trigger"
        local child log pid start ready_ns live_start live_ns
        child="$(make_live_child "live-$payload_name-$label-$i" "$payload" "$ready" "$trigger")"
        log="$tmpdir/live-$payload_name-$label-$i.log"
        start="$(date +%s%N)"
        launch_terminal "$label" "$child" "$log"
        pid="$!"
        if ! wait_ready "$ready" "$pid"; then
            printf '    run %d: fail before ready\n' "$i"
            sed -n '1,20p' "$log"
            kill "$pid" 2>/dev/null || true
            wait "$pid" 2>/dev/null || true
            continue
        fi
        ready_ns=$(($(date +%s%N) - start))
        sleep 0.05
        live_start="$(date +%s%N)"
        : > "$trigger"
        if wait_for_exit "$pid" && wait "$pid"; then
            live_ns=$(($(date +%s%N) - live_start))
            ok=$((ok + 1))
            total_ready_ns=$((total_ready_ns + ready_ns))
            total_live_ns=$((total_live_ns + live_ns))
            printf '    run %d: ready %.2fms live %.2fms ok\n' \
                "$i" \
                "$(awk -v ns="$ready_ns" 'BEGIN { printf "%.2f", ns / 1000000 }')" \
                "$(awk -v ns="$live_ns" 'BEGIN { printf "%.2f", ns / 1000000 }')"
        else
            printf '    run %d: fail during live payload\n' "$i"
            sed -n '1,20p' "$log"
            kill "$pid" 2>/dev/null || true
            wait "$pid" 2>/dev/null || true
        fi
    done

    if ((ok > 0)); then
        awk -v ready="$total_ready_ns" -v live="$total_live_ns" -v ok="$ok" -v bytes="$payload_bytes" 'BEGIN {
            avg_live = live / ok
            seconds = avg_live / 1000000000
            mib = bytes / 1048576
            printf "    avg ready: %.2fms, avg live: %.2fms, live %.2f MiB/s\n", ready / ok / 1000000, avg_live / 1000000, mib / seconds
        }'
    fi
}

printf 'building release terminal...\n'
cargo build --release -p termite >/dev/null

ansi_payload="$(make_payload ansi "$lines")"
unicode_payload="$(make_payload unicode "$lines")"
huge_payload="$(make_payload huge "$huge_lines")"

terminals=(termite foot kitty alacritty)

printf '\n== startup to child-ready, then exit ==\n'
for terminal in "${terminals[@]}"; do
    measure_startup "$terminal"
done

printf '\n== live ansi (%s bytes, startup excluded) ==\n' "$(wc -c < "$ansi_payload")"
for terminal in "${terminals[@]}"; do
    measure_live_payload "$terminal" ansi "$ansi_payload" "$(wc -c < "$ansi_payload")"
done

printf '\n== live unicode (%s bytes, startup excluded) ==\n' "$(wc -c < "$unicode_payload")"
for terminal in "${terminals[@]}"; do
    measure_live_payload "$terminal" unicode "$unicode_payload" "$(wc -c < "$unicode_payload")"
done

printf '\n== live huge (%s bytes, startup excluded) ==\n' "$(wc -c < "$huge_payload")"
for terminal in "${terminals[@]}"; do
    measure_live_payload "$terminal" huge "$huge_payload" "$(wc -c < "$huge_payload")"
done
