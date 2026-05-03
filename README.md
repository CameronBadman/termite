# c-term

Small Rust terminal emulator prototype.

The workspace has two crates:

- `c_term_core`: terminal parser, grid state, and damage tracking.
- `c_term_app`: PTY process management, in-process plugins, and a `winit` window rendered through
  `pixels`/`wgpu`.

## Run

```bash
cargo run -p c_term_app
```

The default executable opens a new window, launches `$SHELL` in a PTY, feeds PTY bytes through the
terminal core, and draws the resulting grid into a GPU-backed pixel surface. Rendering is
event-driven: PTY output, keyboard input, resize, and compositor redraw requests drive frames.

Exit by leaving the shell normally (`exit` or Ctrl-D). Ctrl-Q is also handled as an emergency quit.

This is still an early renderer: glyphs are drawn with an 8x8 bitmap font into a `wgpu` pixel
framebuffer, not a proper shaped text/glyph-atlas pipeline yet.

The app includes a small built-in cursor-line plugin in `crates/c_term_app/src/plugins.rs`.

## Compiled Config

Configuration is Rust code. Edit `crates/c_term_app/src/config.rs`, then rebuild.

Plugins and plugin groups are composed with `Runner::with(...)`. A group can return `impl
RunnerPart`, so local config can be split across small functions instead of one large list.

```rust
pub(crate) fn runner() -> Runner {
    Runner::new().with(default_plugins())
}

fn default_plugins() -> impl RunnerPart {
    parts()
        .with(cursor_line())
        .with(cursor_trail())
}

fn cursor_line() -> CursorLine {
    CursorLine::new(CursorLineConfig {
        row_color: [32, 80, 96],
        row_alpha: 48,
        cell_color: [255, 205, 96],
        cell_alpha: 64,
    })
}
```

`parts()` can be nested, so presets can be split by area:

```rust
fn daily_driver() -> impl RunnerPart {
    parts()
        .with(visuals())
        .with(my_local_plugins())
}
```
