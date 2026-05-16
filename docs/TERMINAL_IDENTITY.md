# Terminal Identity And Clipboard

Terminal identity is the contract Termite presents to shells, tmux, editors, and
other TUIs.

## Current Identity

Termite currently launches child shells with:

```text
TERM=xterm-256color
TERM_PROGRAM=termite
TERM_PROGRAM_VERSION=0.1.0
```

`TERM=xterm-256color` is deliberate. It is conservative and widely available.
Switching to `TERM=termite` before users have installed a terminfo entry would
make basic programs fail or degrade.

Termite-specific identity is centralized in `termite_core::identity`.

That module defines:

- advertised `TERM`
- program name
- program version
- primary device attributes reply
- secondary device attributes reply
- keyboard protocol query reply
- default background color reply
- version reply

## Query Responses

Termite answers common terminal queries in one place instead of scattering byte
strings through the parser.

Current behavior includes:

- Primary DA: `CSI ? 1 ; 2 c`
- Secondary DA: `CSI > 0 ; 0 ; 0 c`
- keyboard protocol query: disabled
- OSC 11 background query: configured default-style reply
- XTVERSION-style reply: `termite <version>`

The exact identity should stay conservative until the emulator supports more of
the surface area that applications infer from these replies.

## Terminfo

A draft terminfo source lives at:

```text
terminfo/termite.terminfo
```

It currently extends `xterm-256color` and adds Termite-relevant extensions such
as OSC 52 clipboard support:

```bash
tic -x terminfo/termite.terminfo
```

Installing it does not automatically make Termite use `TERM=termite`. That
switch should happen only when the terminfo is good enough and the project wants
to require installation.

## Clipboard

Termite supports two clipboard paths.

User shortcut path:

- Ctrl-Shift-C copies Termite's current selection.
- Ctrl-Shift-V pastes Wayland clipboard text into the PTY.
- If bracketed paste is active, paste is wrapped in bracketed paste markers.

OSC 52 path:

- Applications emit `OSC 52`.
- `termite_core` parses the sequence into a clipboard event.
- `termite` decodes the base64 payload.
- The decoded text is written to the Wayland clipboard.

Tmux uses the OSC 52 path for copy mode. Tmux 3.5a enables clipboard support for
`xterm*` clients by default, so Termite's `TERM=xterm-256color` is enough unless
a local tmux config disables clipboard handling.

## Tmux Checks

Inside tmux:

```bash
tmux show -g set-clipboard
tmux show -g terminal-features
```

Useful expected values:

```text
set-clipboard on
terminal-features[0] xterm*:clipboard:...
```

A direct copy smoke test:

```bash
tmux set-buffer -w termite-test
```

After that, paste outside tmux. If `termite-test` appears, OSC 52 copy-out is
working.
