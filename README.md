# Termite

![Termite logo](logo)

Termite is a personal Wayland terminal emulator written in Rust. It is built
around a small terminal core, a `wgpu` renderer, compiled Rust configuration,
and in-process visual plugins.

The project is intentionally opinionated. The goal is not to clone every
feature from every terminal emulator; the goal is a fast, readable terminal that
is easy to profile, hack on, and shape around a personal workflow.

Licensed under GPL-3.0-only. See [LICENSE](LICENSE).

## Highlights

- Wayland windowing through `winit`
- GPU presentation through `wgpu`
- PTY-backed shell with resize support
- Event-driven redraws and low idle CPU usage
- Scrollback with mouse wheel and Shift-PageUp / Shift-PageDown
- Mouse selection, Ctrl-Shift-C copy, and Ctrl-Shift-V paste
- Mouse reporting for tmux, nvim, and other terminal UI applications
- OSC 52 clipboard writes, including tmux copy mode
- Truecolor, indexed colors, basic style attributes, and alternate screen
- TTF font rendering with fallback stacks, symbol fallback, and zoom
- Compiled Rust config for fonts, metrics, theme, opacity, and plugins
- Built-in cursor trail, cursor line/cell, and screen opacity plugins
- Core replay benchmarks and live terminal comparison scripts

## Status

Termite is usable for day-to-day shell, tmux, and editor work on Wayland. It is
still a young terminal, so correctness and performance are developed together
with targeted tests and benchmarks.

Current gaps:

- Wayland only
- no scrollback search yet
- no custom keybinding config yet
- no full terminal conformance claim
- `TERM` currently defaults to `xterm-256color`

## Quick Start

Build and run the terminal:

```bash
cargo run --release -p termite
```

Exit the shell normally with `exit` or Ctrl-D. Ctrl-Q is also handled as an
emergency quit.

Run the test suite:

```bash
cargo test --workspace
```

Run clippy with the same strictness used during development:

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

## Controls

| Action | Input |
| --- | --- |
| Copy selected text | Ctrl-Shift-C |
| Paste clipboard | Ctrl-Shift-V |
| Zoom in | Ctrl-= or Ctrl-+ |
| Zoom out | Ctrl-- |
| Reset zoom | Ctrl-0 |
| Scroll history | Shift-PageUp / Shift-PageDown |
| Select text | Mouse drag |
| Select text while an app tracks mouse | Shift-drag |
| Quit Termite | Ctrl-Q |

Mouse wheel scrolls history when the active application has not enabled mouse
tracking. When tmux, nvim, or another TUI enables mouse tracking, wheel and
click events are sent to the application.

## Configuration

Termite uses compiled Rust configuration instead of a runtime config file. The
main config lives in:

```text
crates/termite/src/config.rs
```

The default config composes font choice, cell metrics, theme colors, text
rendering, zoom behavior, opacity, and visual plugins:

```rust
pub(crate) fn runner() -> Runner {
    Runner::new()
        .with(terminal_font())
        .with(terminal_metrics(terminal_default_metrics()))
        .with(terminal_theme())
        .with(terminal_text_render())
        .with(terminal_zoom_config())
        .with(terminal_plugins())
}
```

This keeps configuration type-checked and explicit. The tradeoff is that config
changes require a rebuild.

See [Compiled Config And Plugins](docs/CONFIG_AND_PLUGINS.md) for the plugin
API, font stack options, theme structure, and zoom behavior.

## Architecture

The workspace is split into two crates:

- `termite_core`: VTE parsing, grid state, scrollback, terminal modes, terminal
  identity, and damage tracking.
- `termite`: Wayland windowing, PTY integration, input encoding, clipboard,
  render cache, GPU presentation, compiled config, and plugins.

PTY bytes flow into `TerminalCore`. The core returns a `CoreTick` containing
grid damage, terminal responses, and clipboard events. The app converts damage
into dirty rows, updates the terminal texture, and presents with `wgpu`.

See [Architecture](docs/ARCHITECTURE.md) for more detail.

## Performance

Termite is performance-focused and has lightweight benchmarking tools checked
into the repo.

Core parser/grid replay:

```bash
cargo run --release -p termite_core --bin core_perf
```

End-to-end terminal comparison suite:

```bash
scripts/bench_terminals.sh
```

Useful knobs:

```bash
RUNS=3 LINES=30000 HUGE_LINES=120000 IDLE_SECONDS=10 scripts/bench_terminals.sh
```

Live runtime timing:

```bash
TERMITE_PERF=1 cargo run --release -p termite
```

`TERMITE_PERF` reports PTY throughput, frame counts, texture upload counts,
plugin work, GPU submission time, and total render time. See
[Performance](docs/PERFORMANCE.md) for the full benchmark workflow.

## Tmux Clipboard

Tmux copy mode can copy out through OSC 52. Termite handles that sequence and
writes the decoded text to the Wayland clipboard.

For tmux 3.5a, the default `xterm*:clipboard` feature is enough when Termite is
running as `TERM=xterm-256color`. If a local tmux config has disabled clipboard
handling, restore it with:

```tmux
set -g set-clipboard on
set -as terminal-features ',xterm-256color:clipboard'
```

After rebuilding Termite, restart the Termite window so tmux sees the new binary
and terminal behavior.

## Terminfo

Termite currently launches shells with `TERM=xterm-256color` plus
`TERM_PROGRAM=termite`. That keeps common tools working without requiring a
custom terminfo entry.

A draft terminfo source is included for later experimentation:

```bash
tic -x terminfo/termite.terminfo
```

See [Terminal Identity And Clipboard](docs/TERMINAL_IDENTITY.md) for details.

## Documentation

- [Architecture](docs/ARCHITECTURE.md)
- [Compiled Config And Plugins](docs/CONFIG_AND_PLUGINS.md)
- [Terminal Identity And Clipboard](docs/TERMINAL_IDENTITY.md)
- [Performance](docs/PERFORMANCE.md)
