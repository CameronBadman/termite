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

## Config

Optional config is read from `$C_TERM_CONFIG`, then `$XDG_CONFIG_HOME/c-term/config`, then
`~/.config/c-term/config`. Lines starting with `#` are comments.

The repo includes a ready-to-copy `config.example`.

```conf
plugin cursor_line
plugin cursor_trail
cursor_trail_hold_ms 80
cursor_trail_decay_ms 320
cursor_trail_threshold 2
cursor_trail_color #ffcd60
```
