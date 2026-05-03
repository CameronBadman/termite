# c-term

Greenfield Rust Wayland terminal emulator architecture scaffold.

The workspace is split around the major ownership boundaries:

- `c_term_core`: event-driven terminal state, damage tracking, and typed terminal events.
- `c_term_renderer`: renderer pipeline contracts, frame policy, clipping, and GPU resource ownership model.
- `c_term_plugins_api`: stable ABI-facing plugin contract.
- `c_term_plugins_sdk`: ergonomic Rust-side plugin authoring wrapper over the ABI concepts.
- `c_term_plugins_host`: plugin registry, subscription fast path, event dispatch, and draw ordering.
- `c_term_app`: app-level orchestration between core, renderer, and plugins.

External integrations such as `smithay-client-toolkit`, EGL/OpenGL, `cosmic-text`, `vte`, and
`libloading` are intentionally behind local traits/adapter boundaries in this scaffold so the
architecture builds without network-fetched dependencies.

## Run

```bash
cargo run -p c_term_app
```

The current executable is a runnable PTY MVP: it launches `$SHELL`, switches the host terminal into
raw alternate-screen mode, forwards keyboard input to the PTY, feeds PTY output through the core/app
pipeline, and writes the child terminal stream to the host terminal.

Exit by leaving the shell normally (`exit` or Ctrl-D). Ctrl-Q is also handled by the MVP as an
emergency quit.

This is not the Wayland/EGL/OpenGL window backend yet; that remains the next renderer backend.
# c-term
