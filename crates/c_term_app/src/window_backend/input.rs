use c_term_core::{MouseState, MouseTracking};
use winit::{
    dpi::PhysicalPosition,
    event::{KeyEvent, MouseButton, MouseScrollDelta},
    keyboard::{Key, ModifiersState, NamedKey},
};

use super::{CELL_HEIGHT, CELL_WIDTH};

pub(super) fn encode_window_key(event: &KeyEvent, modifiers: ModifiersState) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    if modifiers.alt_key() {
        bytes.push(0x1b);
    }

    match event.logical_key.as_ref() {
        Key::Character(ch) if modifiers.control_key() => {
            let mut chars = ch.chars();
            if let Some(ch) = chars.next().filter(|_| chars.next().is_none()) {
                bytes.push(ctrl_byte(ch)?);
            } else {
                return None;
            }
        }
        Key::Character(ch) => bytes.extend_from_slice(ch.as_bytes()),
        Key::Named(NamedKey::Space) if modifiers.control_key() => bytes.push(0x00),
        Key::Named(NamedKey::Space) => bytes.push(b' '),
        Key::Named(NamedKey::Enter) => bytes.push(b'\r'),
        Key::Named(NamedKey::Backspace) => bytes.push(0x7f),
        Key::Named(NamedKey::Tab) => bytes.push(b'\t'),
        Key::Named(NamedKey::Escape) => bytes.push(0x1b),
        Key::Named(NamedKey::ArrowLeft) => bytes.extend_from_slice(b"\x1b[D"),
        Key::Named(NamedKey::ArrowRight) => bytes.extend_from_slice(b"\x1b[C"),
        Key::Named(NamedKey::ArrowUp) => bytes.extend_from_slice(b"\x1b[A"),
        Key::Named(NamedKey::ArrowDown) => bytes.extend_from_slice(b"\x1b[B"),
        Key::Named(NamedKey::Home) => bytes.extend_from_slice(b"\x1b[H"),
        Key::Named(NamedKey::End) => bytes.extend_from_slice(b"\x1b[F"),
        Key::Named(NamedKey::PageUp) => bytes.extend_from_slice(b"\x1b[5~"),
        Key::Named(NamedKey::PageDown) => bytes.extend_from_slice(b"\x1b[6~"),
        Key::Named(NamedKey::Delete) => bytes.extend_from_slice(b"\x1b[3~"),
        Key::Named(NamedKey::Insert) => bytes.extend_from_slice(b"\x1b[2~"),
        _ => return None,
    }

    Some(bytes)
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
) -> Option<(u16, u16)> {
    if position.x < 0.0 || position.y < 0.0 {
        return None;
    }
    let x = (position.x as u32 / CELL_WIDTH) as u16;
    let y = (position.y as u32 / CELL_HEIGHT) as u16;
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

pub(super) fn wheel_scroll_lines(delta: MouseScrollDelta) -> Vec<isize> {
    let raw = match delta {
        MouseScrollDelta::LineDelta(_, y) => f64::from(y),
        MouseScrollDelta::PixelDelta(position) => position.y / f64::from(CELL_HEIGHT),
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
}
