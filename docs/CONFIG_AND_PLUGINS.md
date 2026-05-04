# Compiled Config And Plugins

Termite does not use a runtime config file. Configuration is Rust code in
`crates/c_term_app/src/config.rs`.

This makes configuration explicit and type-checked. The tradeoff is that changes
require a rebuild.

## Runner

The app starts from a `Runner`:

```rust
pub(crate) fn runner() -> Runner {
    Runner::new()
        .with(terminal_font())
        .with(terminal_theme())
        .with(terminal_plugins())
}
```

`RunnerPart` is the composition trait. Anything that implements `RunnerPart`
can configure the runner. Plugins also install directly because `Plugin`
implements `RunnerPart`.

## Presets

Use `parts()` to group configuration:

```rust
fn daily_driver() -> impl RunnerPart {
    parts()
        .with(terminal_font())
        .with(terminal_theme())
        .with(visual_plugins())
}
```

Groups can be nested, which keeps personal presets small without introducing a
separate config language.

## Fonts

The current config supports:

- bitmap cell rendering with `bitmap_font()`
- TTF glyph rasterization with `font_file(path)`
- TTF glyph rasterization with an explicit size through `font_file_with_size`

The default config uses bitmap rendering:

```rust
const USE_TTF_FONT: bool = false;
```

Switching `USE_TTF_FONT` to `true` uses the configured TTF path.

## Theme

Theme colors are real terminal colors, not a post-process tint. A `Theme`
defines:

- default foreground
- default background
- 16 ANSI colors

Example:

```rust
fn terminal_theme() -> impl RunnerPart {
    theme(Theme {
        foreground: [224, 228, 232],
        background: [10, 12, 16],
        ansi: [
            [12, 12, 12],
            [230, 75, 95],
            [82, 196, 120],
            [229, 181, 103],
            [91, 156, 235],
            [190, 118, 235],
            [74, 207, 207],
            [210, 214, 220],
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

Plugins receive the active theme through `PluginFrame`, so visual plugins can
match terminal colors.

## Plugin API

Plugins implement:

```rust
pub(crate) trait Plugin {
    fn draw(&mut self, frame: &mut PluginFrame<'_>) -> bool;
}
```

`draw` is called during render. It can:

- inspect the grid
- inspect current time
- read theme colors
- emit overlay rectangles/quads/rings
- set screen opacity
- return `true` to request another animation frame

Plugins should not mutate terminal state. They are visual extensions over the
current frame.

## Built-In Plugins

`ScreenOpacity`

Sets compositor-visible surface opacity. This is real Wayland/window alpha, not
a fake tint.

`CursorLine`

Small example plugin that highlights the cursor row and cursor cell.

`CursorTrail`

Animated cursor trail inspired by kitty's GPLv3 cursor trail behavior. It uses
theme-aware automatic color by default.

## Adding A Plugin

1. Add a module under `crates/c_term_app/src/plugins/`.
2. Implement `Plugin`.
3. Export it from `plugins.rs`.
4. Add it to `terminal_plugins()` in `config.rs`.
5. Rebuild.

Keep plugin state local to the plugin. If a feature needs to affect PTY input,
terminal parsing, clipboard, or window lifecycle, it probably belongs in
`window_backend` or `c_term_core`, not in a visual plugin.
