pub const TERM: &str = "xterm-256color";
pub const PROGRAM: &str = "termite";
pub const PROGRAM_VERSION: &str = env!("CARGO_PKG_VERSION");

pub const PRIMARY_DEVICE_ATTRIBUTES: &[u8] = b"\x1b[?1;2c";
pub const SECONDARY_DEVICE_ATTRIBUTES: &[u8] = b"\x1b[>0;0;0c";
pub const KEYBOARD_PROTOCOL_QUERY: &[u8] = b"\x1b[?0u";
pub const DEFAULT_BACKGROUND_REPLY: &[u8] = b"\x1b]11;rgb:1010/1212/1818\x1b\\";

pub fn version_reply() -> Vec<u8> {
    format!("\x1bP>|{PROGRAM} {PROGRAM_VERSION}\x1b\\").into_bytes()
}
