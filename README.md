# c-term

Greenfield Rust Wayland terminal emulator architecture scaffold.

The workspace is split around the major ownership boundaries:

- `c_term_core`: event-driven terminal state, damage tracking, and typed terminal events.
- `c_term_renderer`: renderer pipeline contracts, frame policy, clipping, and GPU resource ownership model.
- `c_term_plugins_api`: stable ABI-facing plugin contract.
- `c_term_plugins_sdk`: ergonomic Rust-side plugin authoring wrapper over the ABI concepts.
- `c_term_plugins_host`: plugin registry, subscription fast path, event dispatch, and draw ordering.
- `c_term_app`: app-level orchestration between core, renderer, and plugins.

The current parser uses `vte`. The first window backend uses `winit` plus `pixels`/`wgpu` to open a
Wayland/X11 window and present a GPU-backed framebuffer.

## Run

```bash
cargo run -p c_term_app
```

The default executable opens a new window, launches `$SHELL` in a PTY, feeds PTY bytes through the
terminal core, and draws the resulting grid into a GPU-backed pixel surface. Rendering is
event-driven: PTY output, keyboard input, resize, and compositor redraw requests drive frames.

Exit by leaving the shell normally (`exit` or Ctrl-D). Ctrl-Q is also handled as an emergency quit.

The previous in-terminal backend is still available for debugging:

```bash
cargo run -p c_term_app -- --host
```

This is still an early renderer: glyphs are drawn with an 8x8 bitmap font into a `wgpu` pixel
framebuffer, not a proper shaped text/glyph-atlas pipeline yet.
