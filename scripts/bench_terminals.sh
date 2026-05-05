#!/usr/bin/env bash
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
runs="${RUNS:-3}"
lines="${LINES:-30000}"
huge_lines="${HUGE_LINES:-120000}"
idle_seconds="${IDLE_SECONDS:-10}"
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

make_child() {
    local name="$1"
    local command="$2"
    local file="$tmpdir/$name-child"
    {
        printf '#!/usr/bin/env bash\n'
        printf 'set -euo pipefail\n'
        printf '%s\n' "$command"
    } > "$file"
    chmod +x "$file"
    printf '%s' "$file"
}

measure_terminal() {
    local label="$1"
    local child="$2"
    local total_ns=0
    local ok=0
    shift 2

    printf '  %s\n' "$label"
    for i in $(seq 1 "$runs"); do
        local log="$tmpdir/${label// /_}-$i.log"
        local start end elapsed_ns elapsed_ms status
        start="$(date +%s%N)"
        if timeout "$timeout_s" "$@" "$child" >"$log" 2>&1; then
            status="ok"
            ok=$((ok + 1))
        else
            status="fail"
        fi
        end="$(date +%s%N)"
        elapsed_ns=$((end - start))
        elapsed_ms="$(awk -v ns="$elapsed_ns" 'BEGIN { printf "%.2f", ns / 1000000 }')"
        printf '    run %d: %sms %s\n' "$i" "$elapsed_ms" "$status"
        if [[ "$status" == "ok" ]]; then
            total_ns=$((total_ns + elapsed_ns))
        else
            sed -n '1,20p' "$log"
        fi
    done

    if ((ok > 0)); then
        awk -v ns="$total_ns" -v ok="$ok" 'BEGIN {
            avg = ns / ok
            printf "    avg: %.2fms\n", avg / 1000000
        }'
    fi
}

run_workload() {
    local name="$1"
    local bytes="$2"
    local child="$3"

    printf '\n== %s (%s bytes) ==\n' "$name" "$bytes"
    measure_terminal "termite" "$child" env SHELL="$child" "$repo/target/release/c_term_app"
    if command -v foot >/dev/null 2>&1; then
        measure_terminal "foot" "$child" foot --app-id termite-foot-bench --title termite-foot-bench
    fi
    if command -v kitty >/dev/null 2>&1; then
        measure_terminal "kitty" "$child" kitty --config NONE --class termite-kitty-bench --title termite-kitty-bench
    fi
    if command -v alacritty >/dev/null 2>&1; then
        measure_terminal "alacritty" "$child" alacritty --class termite-alacritty-bench --title termite-alacritty-bench -e
    fi
}

sample_idle() {
    local label="$1"
    local child="$2"
    shift 2
    local log="$tmpdir/idle-$label.log"
    local pid

    "$@" "$child" >"$log" 2>&1 &
    pid="$!"
    sleep 1
    printf '  %s pid=%s\n' "$label" "$pid"
    printf '    sample cpu%% rss_kib threads\n'
    for _ in $(seq 1 "$idle_seconds"); do
        if ! kill -0 "$pid" 2>/dev/null; then
            break
        fi
        sample_process_delta "$pid"
    done
    wait "$pid" || true
}

sample_process_delta() {
    local pid="$1"
    local start_proc start_total end_proc end_total rss threads

    start_proc="$(process_ticks "$pid")" || return 0
    start_total="$(total_ticks)"
    sleep 1
    end_proc="$(process_ticks "$pid")" || return 0
    end_total="$(total_ticks)"
    rss="$(awk '/VmRSS:/ { print $2 }' "/proc/$pid/status" 2>/dev/null || printf '0')"
    threads="$(awk '/Threads:/ { print $2 }' "/proc/$pid/status" 2>/dev/null || printf '0')"

    awk \
        -v start_proc="$start_proc" \
        -v end_proc="$end_proc" \
        -v start_total="$start_total" \
        -v end_total="$end_total" \
        -v rss="$rss" \
        -v threads="$threads" \
        'BEGIN {
            proc_delta = end_proc - start_proc
            total_delta = end_total - start_total
            cpu_count = system_cpu_count()
            cpu = total_delta > 0 ? proc_delta * 100.0 * cpu_count / total_delta : 0
            printf "    %6.2f %7d %7d\n", cpu, rss, threads
        }
        function system_cpu_count(    n, line) {
            while ((getline line < "/proc/cpuinfo") > 0) {
                if (line ~ /^processor[ \t]*:/) n++
            }
            close("/proc/cpuinfo")
            return n > 0 ? n : 1
        }'
}

process_ticks() {
    local pid="$1"
    awk '{ print $14 + $15 }' "/proc/$pid/stat"
}

total_ticks() {
    awk '/^cpu / {
        total = 0
        for (i = 2; i <= NF; i++) total += $i
        print total
    }' /proc/stat
}

gpu_idle_note() {
    printf '\n== gpu idle ==\n'
    if command -v nvidia-smi >/dev/null 2>&1; then
        printf 'nvidia-smi available; add NVIDIA sampling here if needed.\n'
    elif command -v intel_gpu_top >/dev/null 2>&1; then
        printf 'intel_gpu_top available; add Intel sampling here if needed.\n'
    elif command -v radeontop >/dev/null 2>&1; then
        printf 'radeontop available; add AMD sampling here if needed.\n'
    else
        printf 'No supported GPU sampler found: nvidia-smi, intel_gpu_top, radeontop unavailable.\n'
        printf 'CPU/RSS idle samples above are valid; GPU idle needs one of those tools installed.\n'
    fi
}

printf 'building release terminal...\n'
cargo build --release -p c_term_app >/dev/null

printf '\n== core replay ==\n'
cargo run --release -p c_term_core --bin core_perf

ansi_payload="$(make_payload ansi "$lines")"
unicode_payload="$(make_payload unicode "$lines")"
huge_payload="$(make_payload huge "$huge_lines")"

ansi_child="$(make_child ansi "cat $(printf '%q' "$ansi_payload")")"
unicode_child="$(make_child unicode "cat $(printf '%q' "$unicode_payload")")"
huge_child="$(make_child huge "cat $(printf '%q' "$huge_payload")")"
tmux_child="$(make_child tmux "tmux -L termite-bench-$$ -f /dev/null new-session 'cat $(printf '%q' "$ansi_payload")'")"
nvim_child="$(make_child nvim "nvim --clean -n -u NONE '+set nomore' '+edit $(printf '%q' "$ansi_payload")' '+normal! G' '+qa!'")"

run_workload "ansi scroll" "$(wc -c < "$ansi_payload")" "$ansi_child"
run_workload "unicode output" "$(wc -c < "$unicode_payload")" "$unicode_child"
run_workload "huge burst" "$(wc -c < "$huge_payload")" "$huge_child"
run_workload "tmux ansi" "$(wc -c < "$ansi_payload")" "$tmux_child"
run_workload "nvim open+goto-end" "$(wc -c < "$ansi_payload")" "$nvim_child"

printf '\n== idle cpu/rss (%ss) ==\n' "$idle_seconds"
idle_child="$(make_child idle "sleep $((idle_seconds + 2))")"
sample_idle "termite" "$idle_child" env SHELL="$idle_child" "$repo/target/release/c_term_app"
if command -v foot >/dev/null 2>&1; then
    sample_idle "foot" "$idle_child" foot --app-id termite-foot-idle --title termite-foot-idle
fi
if command -v kitty >/dev/null 2>&1; then
    sample_idle "kitty" "$idle_child" kitty --config NONE --class termite-kitty-idle --title termite-kitty-idle
fi
if command -v alacritty >/dev/null 2>&1; then
    sample_idle "alacritty" "$idle_child" alacritty --class termite-alacritty-idle --title termite-alacritty-idle -e
fi
gpu_idle_note
