use termite_core::{MouseState, MouseTracking};
use winit::{
    dpi::PhysicalPosition,
    event::{KeyEvent, MouseButton, MouseScrollDelta},
    keyboard::{Key, ModifiersState, NamedKey},
};

use crate::runner::TerminalMetrics;

pub(super) fn encode_window_key(event: &KeyEvent, modifiers: ModifiersState) -> Option<Vec<u8>> {
    match event.logical_key.as_ref() {
        Key::Character(ch) => encode_character_key(ch, modifiers),
        Key::Named(key) => encode_named_key(&key, modifiers),
        _ => None,
    }
}

fn encode_character_key(ch: &str, modifiers: ModifiersState) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    if modifiers.alt_key() {
        bytes.push(0x1b);
    }

    if modifiers.control_key() {
        let mut chars = ch.chars();
        if let Some(ch) = chars.next().filter(|_| chars.next().is_none()) {
            bytes.push(ctrl_byte(ch)?);
        } else {
            return None;
        }
    } else {
        bytes.extend_from_slice(ch.as_bytes());
    }
    Some(bytes)
}

fn encode_named_key(key: &NamedKey, modifiers: ModifiersState) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    let alt_prefix = matches!(
        key,
        NamedKey::Space | NamedKey::Enter | NamedKey::Backspace | NamedKey::Escape
    ) && modifiers.alt_key();
    if alt_prefix {
        bytes.push(0x1b);
    }

    match key {
        NamedKey::Space if modifiers.control_key() => bytes.push(0x00),
        NamedKey::Space => bytes.push(b' '),
        NamedKey::Enter => bytes.push(b'\r'),
        NamedKey::Backspace => bytes.push(0x7f),
        NamedKey::Tab if modifiers.shift_key() => return Some(b"\x1b[Z".to_vec()),
        NamedKey::Tab => bytes.push(b'\t'),
        NamedKey::Escape => bytes.push(0x1b),
        NamedKey::ArrowLeft => return Some(csi_modified_final('D', modifiers)),
        NamedKey::ArrowRight => return Some(csi_modified_final('C', modifiers)),
        NamedKey::ArrowUp => return Some(csi_modified_final('A', modifiers)),
        NamedKey::ArrowDown => return Some(csi_modified_final('B', modifiers)),
        NamedKey::Home => return Some(csi_modified_final('H', modifiers)),
        NamedKey::End => return Some(csi_modified_final('F', modifiers)),
        NamedKey::PageUp => return Some(csi_numbered_key(5, modifiers)),
        NamedKey::PageDown => return Some(csi_numbered_key(6, modifiers)),
        NamedKey::Delete => return Some(csi_numbered_key(3, modifiers)),
        NamedKey::Insert => return Some(csi_numbered_key(2, modifiers)),
        NamedKey::F1 => return Some(function_key(1, modifiers)),
        NamedKey::F2 => return Some(function_key(2, modifiers)),
        NamedKey::F3 => return Some(function_key(3, modifiers)),
        NamedKey::F4 => return Some(function_key(4, modifiers)),
        NamedKey::F5 => return Some(function_key(5, modifiers)),
        NamedKey::F6 => return Some(function_key(6, modifiers)),
        NamedKey::F7 => return Some(function_key(7, modifiers)),
        NamedKey::F8 => return Some(function_key(8, modifiers)),
        NamedKey::F9 => return Some(function_key(9, modifiers)),
        NamedKey::F10 => return Some(function_key(10, modifiers)),
        NamedKey::F11 => return Some(function_key(11, modifiers)),
        NamedKey::F12 => return Some(function_key(12, modifiers)),
        _ => return None,
    }
    Some(bytes)
}

fn csi_modified_final(final_byte: char, modifiers: ModifiersState) -> Vec<u8> {
    if let Some(code) = modifier_code(modifiers) {
        format!("\x1b[1;{code}{final_byte}").into_bytes()
    } else {
        format!("\x1b[{final_byte}").into_bytes()
    }
}

fn csi_numbered_key(number: u8, modifiers: ModifiersState) -> Vec<u8> {
    if let Some(code) = modifier_code(modifiers) {
        format!("\x1b[{number};{code}~").into_bytes()
    } else {
        format!("\x1b[{number}~").into_bytes()
    }
}

fn function_key(number: u8, modifiers: ModifiersState) -> Vec<u8> {
    let csi_number = match number {
        5 => 15,
        6 => 17,
        7 => 18,
        8 => 19,
        9 => 20,
        10 => 21,
        11 => 23,
        12 => 24,
        _ => 0,
    };
    if csi_number != 0 {
        return csi_numbered_key(csi_number, modifiers);
    }

    let final_byte = match number {
        1 => 'P',
        2 => 'Q',
        3 => 'R',
        4 => 'S',
        _ => return Vec::new(),
    };
    if let Some(code) = modifier_code(modifiers) {
        format!("\x1b[1;{code}{final_byte}").into_bytes()
    } else {
        format!("\x1bO{final_byte}").into_bytes()
    }
}

fn modifier_code(modifiers: ModifiersState) -> Option<u8> {
    let mut code = 1;
    if modifiers.shift_key() {
        code += 1;
    }
    if modifiers.alt_key() {
        code += 2;
    }
    if modifiers.control_key() {
        code += 4;
    }
    (code > 1).then_some(code)
}

pub(super) fn shortcut_key(key: &Key, modifiers: ModifiersState, target: char) -> bool {
    modifiers.control_key()
        && modifiers.shift_key()
        && matches!(
            key.as_ref(),
            Key::Character(ch)
                if ch.chars().next().is_some_and(|ch| ch.eq_ignore_ascii_case(&target))
                    && ch.chars().nth(1).is_none()
        )
}

fn ctrl_byte(ch: char) -> Option<u8> {
    let lower = ch.to_ascii_lowercase();
    if lower.is_ascii_alphabetic() {
        Some((lower as u8) - b'a' + 1)
    } else {
        match lower {
            ' ' | '@' | '2' => Some(0x00),
            '[' => Some(0x1b),
            '3' => Some(0x1b),
            '\\' => Some(0x1c),
            '4' => Some(0x1c),
            ']' => Some(0x1d),
            '5' => Some(0x1d),
            '^' => Some(0x1e),
            '6' => Some(0x1e),
            '_' | '/' | '-' | '7' => Some(0x1f),
            '?' => Some(0x7f),
            '8' => Some(0x7f),
            '`' => Some(0x00),
            '~' => Some(0x1e),
            '=' => Some(0x1f),
            _ => None,
        }
    }
}

pub(super) fn mouse_cell(
    position: PhysicalPosition<f64>,
    cols: u16,
    rows: u16,
    metrics: TerminalMetrics,
) -> Option<(u16, u16)> {
    if position.x < 0.0 || position.y < 0.0 {
        return None;
    }
    let x = (position.x as u32 / metrics.cell_width) as u16;
    let y = (position.y as u32 / metrics.cell_height) as u16;
    (x < cols && y < rows).then_some((x, y))
}

pub(super) fn mouse_button_code(button: MouseButton) -> Option<u8> {
    match button {
        MouseButton::Left => Some(0),
        MouseButton::Middle => Some(1),
        MouseButton::Right => Some(2),
        _ => None,
    }
}

pub(super) fn active_mouse_button(buttons: u8) -> u8 {
    for button in 0..3 {
        if buttons & (1 << button) != 0 {
            return button;
        }
    }
    3
}

pub(super) fn wheel_codes(delta: MouseScrollDelta) -> Vec<u8> {
    match delta {
        MouseScrollDelta::LineDelta(x, y) => wheel_axis_codes(x, y),
        MouseScrollDelta::PixelDelta(position) => {
            wheel_axis_codes(position.x as f32, position.y as f32)
        }
    }
}

fn wheel_axis_codes(x: f32, y: f32) -> Vec<u8> {
    let mut codes = Vec::new();
    if y > 0.0 {
        codes.push(64);
    } else if y < 0.0 {
        codes.push(65);
    }
    if x > 0.0 {
        codes.push(67);
    } else if x < 0.0 {
        codes.push(66);
    }
    codes
}

pub(super) fn wheel_scroll_lines(delta: MouseScrollDelta, metrics: TerminalMetrics) -> Vec<isize> {
    let raw = match delta {
        MouseScrollDelta::LineDelta(_, y) => f64::from(y),
        MouseScrollDelta::PixelDelta(position) => position.y / f64::from(metrics.cell_height),
    };
    let lines = if raw.abs() < 1.0 {
        raw.signum() as isize
    } else {
        raw.round() as isize
    };
    let lines = if lines == 0 { 1 } else { lines };
    vec![lines.clamp(-12, 12)]
}

pub(super) fn encode_mouse_event(
    mouse: MouseState,
    code: u8,
    x: u16,
    y: u16,
    modifiers: ModifiersState,
    release: bool,
) -> Vec<u8> {
    if mouse.tracking == MouseTracking::None {
        return Vec::new();
    }

    let code = code + mouse_modifier_bits(modifiers);
    let col = x.saturating_add(1);
    let row = y.saturating_add(1);
    if mouse.sgr {
        let final_byte = if release { 'm' } else { 'M' };
        format!("\x1b[<{code};{col};{row}{final_byte}").into_bytes()
    } else {
        let Ok(code) = u8::try_from(u16::from(code) + 32) else {
            return Vec::new();
        };
        let Ok(col) = u8::try_from(col.saturating_add(32)) else {
            return Vec::new();
        };
        let Ok(row) = u8::try_from(row.saturating_add(32)) else {
            return Vec::new();
        };
        vec![0x1b, b'[', b'M', code, col, row]
    }
}

fn mouse_modifier_bits(modifiers: ModifiersState) -> u8 {
    let mut bits = 0;
    if modifiers.shift_key() {
        bits |= 4;
    }
    if modifiers.alt_key() {
        bits |= 8;
    }
    if modifiers.control_key() {
        bits |= 16;
    }
    bits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sgr_mouse_encoding_uses_one_based_coordinates() {
        let mouse = MouseState {
            tracking: MouseTracking::Click,
            sgr: true,
        };

        assert_eq!(
            encode_mouse_event(mouse, 0, 2, 3, ModifiersState::empty(), false),
            b"[<0;3;4M"
        );
        assert_eq!(
            encode_mouse_event(mouse, 0, 2, 3, ModifiersState::empty(), true),
            b"[<0;3;4m"
        );
    }

    #[test]
    fn mouse_encoding_includes_modifiers() {
        let mouse = MouseState {
            tracking: MouseTracking::Click,
            sgr: true,
        };

        assert_eq!(
            encode_mouse_event(
                mouse,
                64,
                0,
                0,
                ModifiersState::SHIFT | ModifiersState::CONTROL,
                false,
            ),
            b"[<84;1;1M"
        );
    }

    #[test]
    fn ctrl_shift_shortcuts_match_case_insensitively() {
        assert!(shortcut_key(
            &Key::Character("V".into()),
            ModifiersState::CONTROL | ModifiersState::SHIFT,
            'v'
        ));
    }

    #[test]
    fn navigation_keys_use_xterm_modifier_encoding() {
        assert_eq!(
            encode_named_key(
                &NamedKey::ArrowLeft,
                ModifiersState::CONTROL | ModifiersState::SHIFT,
            ),
            Some(b"\x1b[1;6D".to_vec())
        );
        assert_eq!(
            encode_named_key(&NamedKey::Delete, ModifiersState::ALT),
            Some(b"\x1b[3;3~".to_vec())
        );
        assert_eq!(
            encode_named_key(&NamedKey::Home, ModifiersState::empty()),
            Some(b"\x1b[H".to_vec())
        );
    }

    #[test]
    fn function_keys_use_xterm_sequences() {
        assert_eq!(
            encode_named_key(&NamedKey::F1, ModifiersState::empty()),
            Some(b"\x1bOP".to_vec())
        );
        assert_eq!(
            encode_named_key(&NamedKey::F1, ModifiersState::CONTROL),
            Some(b"\x1b[1;5P".to_vec())
        );
        assert_eq!(
            encode_named_key(&NamedKey::F12, ModifiersState::SHIFT),
            Some(b"\x1b[24;2~".to_vec())
        );
    }

    #[test]
    fn shift_tab_uses_backtab_sequence() {
        assert_eq!(
            encode_named_key(&NamedKey::Tab, ModifiersState::SHIFT),
            Some(b"\x1b[Z".to_vec())
        );
    }
}
