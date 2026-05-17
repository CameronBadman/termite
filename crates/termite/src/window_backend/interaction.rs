use std::io::{Read, Write};

use termite_core::{MouseState, MouseTracking, TerminalCore};
use winit::{
    dpi::PhysicalPosition,
    event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta},
    event_loop::ActiveEventLoop,
    keyboard::{Key, NamedKey},
};
use wl_clipboard_rs::{copy as wl_copy, paste as wl_paste};

use super::{
    WindowBackend,
    input::{
        active_mouse_button, encode_mouse_event, encode_window_key, mouse_button_code, mouse_cell,
        shortcut_key, wheel_codes, wheel_scroll_lines,
    },
    selection::Selection,
};

impl WindowBackend {
    fn scroll_view(&mut self, delta: isize) -> bool {
        let Some(terminal) = &self.terminal else {
            return false;
        };
        if terminal.is_alternate_screen() {
            return false;
        }

        let max_offset = terminal.scrollback_len();
        let next = if delta >= 0 {
            self.scroll_offset.saturating_add(delta as usize)
        } else {
            self.scroll_offset.saturating_sub(delta.unsigned_abs())
        }
        .min(max_offset);

        if next == self.scroll_offset {
            return false;
        }
        self.scroll_offset = next;
        self.selection = None;
        self.mark_dirty();
        true
    }

    fn snap_to_live(&mut self) {
        if self.scroll_offset == 0 {
            return;
        }
        self.scroll_offset = 0;
        self.selection = None;
        self.render_cache.invalidate();
        self.mark_dirty();
    }

    pub(super) fn handle_key(&mut self, event_loop: &ActiveEventLoop, event: KeyEvent) {
        if event.state != ElementState::Pressed {
            return;
        }

        if shortcut_key(&event.logical_key, self.modifiers, 'c') {
            self.copy_selection();
            return;
        }
        if shortcut_key(&event.logical_key, self.modifiers, 'v') {
            self.paste_clipboard();
            return;
        }
        if self.handle_zoom_key(&event.logical_key) {
            return;
        }

        if self.modifiers.control_key() && matches!(event.logical_key.as_ref(), Key::Character("q"))
        {
            event_loop.exit();
            return;
        }

        if self.modifiers.shift_key() {
            match event.logical_key.as_ref() {
                Key::Named(NamedKey::PageUp) => {
                    let lines = self.rows.saturating_sub(1).max(1) as isize;
                    let _ = self.scroll_view(lines);
                    return;
                }
                Key::Named(NamedKey::PageDown) => {
                    let lines = self.rows.saturating_sub(1).max(1) as isize;
                    let _ = self.scroll_view(-lines);
                    return;
                }
                _ => {}
            }
        }

        let Some(bytes) = encode_window_key(&event, self.modifiers) else {
            return;
        };

        self.selection = None;
        self.snap_to_live();
        if let Some(child) = &mut self.child
            && let Err(error) = child.master.write_all(&bytes)
        {
            eprintln!("termite: failed to write key to PTY: {error}");
        }
    }

    pub(super) fn handle_mouse_input(&mut self, state: ElementState, button: MouseButton) {
        let Some(cell) = self.mouse_position else {
            return;
        };
        let Some(button_code) = mouse_button_code(button) else {
            return;
        };

        match state {
            ElementState::Pressed => self.mouse_buttons |= 1 << button_code,
            ElementState::Released => self.mouse_buttons &= !(1 << button_code),
        }

        if button == MouseButton::Left && self.selection_mouse_mode() {
            match state {
                ElementState::Pressed => self.selection = Some(Selection::start(cell)),
                ElementState::Released => {
                    if let Some(selection) = &mut self.selection {
                        selection.finish();
                    }
                }
            }
            self.mark_dirty();
            return;
        }

        let Some(terminal) = &self.terminal else {
            return;
        };
        let mouse = terminal.mouse();
        if mouse.tracking == MouseTracking::None {
            return;
        }

        let code = if state == ElementState::Pressed {
            button_code
        } else {
            3
        };
        self.write_mouse_event(mouse, code, cell, state == ElementState::Released);
    }

    pub(super) fn handle_mouse_move(&mut self, position: PhysicalPosition<f64>) {
        let Some(cell) = mouse_cell(position, self.cols, self.rows, self.metrics) else {
            self.mouse_position = None;
            return;
        };
        if self.mouse_position == Some(cell) {
            return;
        }
        self.mouse_position = Some(cell);

        if let Some(selection) = &mut self.selection
            && selection.is_dragging()
        {
            if selection.update(cell) {
                self.mark_dirty();
            }
            return;
        }

        let Some(terminal) = &self.terminal else {
            return;
        };
        let mouse = terminal.mouse();
        let code = match mouse.tracking {
            MouseTracking::Any => 35,
            MouseTracking::Drag if self.mouse_buttons != 0 => {
                active_mouse_button(self.mouse_buttons) + 32
            }
            _ => return,
        };
        self.write_mouse_event(mouse, code, cell, false);
    }

    pub(super) fn handle_mouse_wheel(&mut self, delta: MouseScrollDelta) {
        let Some(terminal) = &self.terminal else {
            return;
        };
        let mouse = terminal.mouse();
        if mouse.tracking == MouseTracking::None {
            for lines in wheel_scroll_lines(delta, self.metrics) {
                let _ = self.scroll_view(lines);
            }
            return;
        }

        let Some(cell) = self.mouse_position else {
            return;
        };
        for code in wheel_codes(delta) {
            self.write_mouse_event(mouse, code, cell, false);
        }
    }

    fn write_mouse_event(&mut self, mouse: MouseState, code: u8, cell: (u16, u16), release: bool) {
        let bytes = encode_mouse_event(mouse, code, cell.0, cell.1, self.modifiers, release);
        if bytes.is_empty() {
            return;
        }
        if let Some(child) = &mut self.child
            && let Err(error) = child.master.write_all(&bytes)
        {
            eprintln!("termite: failed to write mouse event to PTY: {error}");
        }
    }

    fn selection_mouse_mode(&self) -> bool {
        self.modifiers.shift_key()
            || self
                .terminal
                .as_ref()
                .is_some_and(|terminal| terminal.mouse().tracking == MouseTracking::None)
    }

    fn copy_selection(&self) {
        let (Some(selection), Some(terminal)) = (self.selection, &self.terminal) else {
            return;
        };
        let text = selection.text(terminal, self.scroll_offset);
        if text.is_empty() {
            return;
        }
        let source = wl_copy::Source::Bytes(text.into_bytes().into_boxed_slice());
        if let Err(error) = wl_copy::Options::new().copy(source, wl_copy::MimeType::Text) {
            eprintln!("termite: failed to copy selection: {error}");
        }
    }

    fn paste_clipboard(&mut self) {
        let (mut pipe, _) = match wl_paste::get_contents(
            wl_paste::ClipboardType::Regular,
            wl_paste::Seat::Unspecified,
            wl_paste::MimeType::Text,
        ) {
            Ok(contents) => contents,
            Err(
                wl_paste::Error::NoSeats
                | wl_paste::Error::ClipboardEmpty
                | wl_paste::Error::NoMimeType,
            ) => return,
            Err(error) => {
                eprintln!("termite: failed to read clipboard: {error}");
                return;
            }
        };
        let mut text = String::new();
        if let Err(error) = pipe.read_to_string(&mut text) {
            eprintln!("termite: failed to read clipboard text: {error}");
            return;
        }
        self.write_paste(&text);
    }

    fn write_paste(&mut self, text: &str) {
        let bracketed = self
            .terminal
            .as_ref()
            .is_some_and(TerminalCore::bracketed_paste);
        let mut bytes = Vec::with_capacity(text.len() + if bracketed { 12 } else { 0 });
        if bracketed {
            bytes.extend_from_slice(b"\x1b[200~");
        }
        bytes.extend(text.bytes().filter(|byte| *byte != 0));
        if bracketed {
            bytes.extend_from_slice(b"\x1b[201~");
        }

        self.selection = None;
        self.snap_to_live();
        if let Some(child) = &mut self.child
            && let Err(error) = child.master.write_all(&bytes)
        {
            eprintln!("termite: failed to paste clipboard text to PTY: {error}");
        }
    }
}
