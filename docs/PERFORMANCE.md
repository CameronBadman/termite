# Performance

Termite has two lightweight performance tools.

## Core Replay Benchmark

Run parser/grid replay benchmarks without opening a window:

```bash
cargo run --release -p termite_core --bin core_perf
```

This prints throughput for plain scrolling, SGR-heavy output, cursor movement,
and Unicode-heavy output. It measures `TerminalCore` only, not GPU rendering.

## Terminal Comparison Suite

Compare Termite and Kitty on several end-to-end workloads:

```bash
scripts/bench_terminals.sh
```

Useful environment knobs:

```bash
RUNS=2 LINES=10000 HUGE_LINES=40000 IDLE_SECONDS=5 scripts/bench_terminals.sh
```

The suite runs core replay, ANSI scroll output, Unicode output, a large burst,
tmux output, nvim open/go-to-end, and idle CPU/RSS sampling. GPU idle sampling
is reported only when a supported local tool is installed, such as
`nvidia-smi`, `intel_gpu_top`, or `radeontop`.

## Live Runtime Timing

Run the app with runtime timing enabled:

```bash
TERMITE_PERF=1 cargo run --release -p termite
```

While the terminal is open, stress it with commands such as:

```bash
scripts/text_smoke.sh
seq 1 200000
find /usr -maxdepth 4 2>/dev/null
```

The launching terminal receives one `termite-perf` line per second with PTY
throughput, frame counts, upload counts, overlay counts, and CPU-side timing for
core processing, render-cache updates, plugins, GPU submission, and total render
work.
