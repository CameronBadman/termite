# Termite

![Termite logo](logo)

Termite is a small Wayland terminal emulator written in Rust.

It is intentionally opinionated: GPU-rendered, event-driven, configured in
Rust, and extended through in-process plugins compiled into the binary. The goal
is not full terminal feature parity. The goal is a fast terminal that is easy to
read, hack on, and shape for a personal workflow.

Licensed under GPL-3.0-only. See `LICENSE`.

## Status

Termite is usable for day-to-day shell, tmux, and editor work on Wayland, but it
is still young.

Works now:

- Wayland window via `winit`
- GPU presentation via `wgpu`
- PTY-backed shell
- event-driven redraws
- scrollback with Shift-PageUp and Shift-PageDown
- mouse reporting for tmux/nvim-style TUIs
- mouse selection, Ctrl-Shift-C copy, Ctrl-Shift-V paste
- OSC 52 clipboard writes, including tmux copy mode
- truecolor, indexed color, basic style attributes, alternate screen
- compiled Rust config for theme, font, opacity, and plugins
- built-in cursor line, screen opacity, and cursor trail plugins

Known gaps:

- Wayland only
- no scrollback search yet
- no custom keybinding config yet
- no full terminal conformance claim
- `TERM` still defaults to `xterm-256color` until custom terminfo is worth
  requiring

## Run

```bash
cargo run --release -p c_term_app
```

Exit the shell normally with `exit` or Ctrl-D. Ctrl-Q is also handled as an
emergency quit.

## Controls

| Action | Input |
| --- | --- |
| Copy selected text | Ctrl-Shift-C |
| Paste clipboard | Ctrl-Shift-V |
| Scroll history | Shift-PageUp / Shift-PageDown |
| Select text | Mouse drag |
| Select text while an app tracks mouse | Shift-drag |
| Quit Termite | Ctrl-Q |

Mouse wheel scrolls history when the active application has not enabled mouse
tracking. When tmux, nvim, or another TUI enables mouse tracking, wheel and
clicks are sent to the application.

## Tmux Clipboard

Tmux copy mode uses OSC 52 to copy out to the host terminal. Termite handles
that sequence and writes to the Wayland clipboard.

For tmux 3.5a, the default `xterm*:clipboard` feature is enough when Termite is
running as `TERM=xterm-256color`. If a local tmux config has disabled clipboard
handling, restore it with:

```tmux
set -g set-clipboard on
set -as terminal-features ',xterm-256color:clipboard'
```

After rebuilding Termite, restart the Termite window so tmux sees the new
binary and terminal behavior.

## Project Shape

The workspace has two crates:

- `c_term_core`: parser, grid state, scrollback, terminal modes, identity, and
  damage tracking.
- `c_term_app`: PTY/window integration, rendering, compiled config, clipboard
  integration, and built-in plugins.

Rendering is event-driven. PTY output, input, resize events, animation timers,
and compositor redraw requests drive frames. The CPU side maintains a terminal
grid and damage model; the app side renders updated rows into a texture and
presents with `wgpu`.

## Compiled Config

Configuration is Rust code in `crates/c_term_app/src/config.rs`.

Edit it, rebuild, run. The default config composes font choice, theme colors,
opacity, and visual plugins:

```rust
pub(crate) fn runner() -> Runner {
    Runner::new()
        .with(terminal_font())
        .with(terminal_theme())
        .with(terminal_plugins())
}
```

Plugins and presets are composed with `parts()`:

```rust
fn terminal_plugins() -> impl RunnerPart {
    parts()
        .with(screen_opacity_plugin())
        .with(cursor_line_plugin())
        .with(cursor_trail_plugin())
}
```

This is closer to dwm-style compiled configuration than runtime config files.

## Docs

- [Architecture](docs/ARCHITECTURE.md)
- [Compiled Config And Plugins](docs/CONFIG_AND_PLUGINS.md)
- [Terminal Identity And Clipboard](docs/TERMINAL_IDENTITY.md)
- [Performance](docs/PERFORMANCE.md)

## Terminfo

Termite currently launches shells with `TERM=xterm-256color` plus
`TERM_PROGRAM=termite`. That keeps basic tools working without installing a
custom terminfo entry.

A draft terminfo source is included for later:

```bash
tic -x terminfo/termite.terminfo
```

See [Terminal Identity And Clipboard](docs/TERMINAL_IDENTITY.md) for the
details.
