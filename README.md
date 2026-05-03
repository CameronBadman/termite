# Termite

![Termite logo](logo)

Termite is a small Wayland terminal emulator written in Rust.

It is intentionally opinionated: GPU-rendered, event-driven, configured in Rust,
and extended through in-process plugins that you compile into the binary. It is
not trying to be every terminal. The goal is a fast, readable terminal that is
easy to hack on.

Licensed under GPL-3.0-only. See `LICENSE`.

## Shape

The workspace has two crates:

- `c_term_core`: parser, grid state, scrollback, terminal modes, and damage
  tracking.
- `c_term_app`: PTY/window integration, rendering, compiled config, and
  built-in plugins.

The app is Wayland-focused. It opens a `winit` window, launches `$SHELL` in a
PTY, processes terminal output through `c_term_core`, and renders the grid with
`wgpu`.

Rendering is event-driven. PTY output, input, resize events, animation timers,
and compositor redraw requests drive frames.

## Run

```bash
cargo run --release -p c_term_app
```

Exit the shell normally with `exit` or Ctrl-D. Ctrl-Q is also handled as an
emergency quit.

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

fn terminal_plugins() -> impl RunnerPart {
    parts()
        .with(screen_opacity_plugin())
        .with(cursor_line_plugin())
        .with(cursor_trail_plugin())
}
```

`parts()` can be nested, so local presets can stay small:

```rust
fn daily_driver() -> impl RunnerPart {
    parts()
        .with(terminal_theme())
        .with(visual_plugins())
}
```

## Theme

Colors are configured before rendering, not as a post-process tint. The theme
sets default foreground/background plus the 16 ANSI colors:

```rust
fn terminal_theme() -> impl RunnerPart {
    theme(Theme {
        foreground: [224, 228, 232],
        background: [10, 12, 16],
        ansi: [
            [12, 12, 12],     // black
            [230, 75, 95],   // red
            [82, 196, 120],  // green
            [229, 181, 103], // yellow
            [91, 156, 235],  // blue
            [190, 118, 235], // magenta
            [74, 207, 207],  // cyan
            [210, 214, 220], // white
            [118, 124, 136],
            [255, 105, 125],
            [115, 225, 145],
            [245, 209, 125],
            [125, 180, 255],
            [215, 145, 255],
            [105, 235, 235],
            [245, 247, 250],
        ],
    })
}
```

## Plugins

Plugins are Rust types that implement `Plugin`. They receive a `PluginFrame`
with access to the current grid, time, theme, overlay commands, and window
opacity.

Built-ins:

- `ScreenOpacity`: makes the compositor-visible Wayland window translucent.
- `CursorLine`: minimal example plugin. It emits a row and cell overlay.
- `CursorTrail`: advanced animated example plugin based on the visual behavior
  of kitty terminal's GPLv3 cursor trail.

The intended workflow is simple: add a plugin module, add it to
`terminal_plugins()`, rebuild.

## Notes

Text rendering is deliberately simple right now. The default path uses an 8x16
bitmap-style cell renderer with optional TTF glyph rasterization. There is no
goal to chase full terminal feature parity unless a feature fits the project.
