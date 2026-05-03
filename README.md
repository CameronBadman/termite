# c-term

Small Rust terminal emulator prototype.

The workspace has two crates:

- `c_term_core`: terminal parser, grid state, damage tracking, and terminal events.
- `c_term_app`: PTY process management plus a `winit` window rendered through `pixels`/`wgpu`.

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
