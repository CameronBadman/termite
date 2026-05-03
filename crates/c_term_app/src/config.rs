use std::{env, fs, path::PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AppConfig {
    pub(crate) plugins: Vec<String>,
    pub(crate) cursor_trail: CursorTrailConfig,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct CursorTrailConfig {
    pub(crate) hold_ms: u64,
    pub(crate) decay_ms: u64,
    pub(crate) threshold: u16,
    pub(crate) length: f32,
    pub(crate) color: [u8; 3],
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            plugins: vec!["cursor_line".into(), "cursor_trail".into()],
            cursor_trail: CursorTrailConfig::default(),
        }
    }
}

impl Default for CursorTrailConfig {
    fn default() -> Self {
        Self {
            hold_ms: 35,
            decay_ms: 520,
            threshold: 2,
            length: 0.82,
            color: [104, 247, 255],
        }
    }
}

impl AppConfig {
    pub(crate) fn load() -> Self {
        let Some(path) = config_path() else {
            return Self::default();
        };
        match fs::read_to_string(path) {
            Ok(text) => Self::parse(&text),
            Err(_) => Self::default(),
        }
    }

    fn parse(text: &str) -> Self {
        let mut config = Self {
            plugins: Vec::new(),
            ..Self::default()
        };

        for line in text.lines().map(str::trim) {
            if line.starts_with('#') {
                continue;
            }
            let mut words = line.split_whitespace();
            let Some(key) = words.next() else {
                continue;
            };
            match key {
                "plugin" => config.plugins.extend(words.map(str::to_owned)),
                "plugins" => {
                    config.plugins.extend(
                        words
                            .flat_map(|word| word.split(','))
                            .filter(|name| !name.is_empty())
                            .map(str::to_owned),
                    );
                }
                "cursor_trail_hold_ms" => {
                    config.cursor_trail.hold_ms =
                        parse_u64(words.next(), config.cursor_trail.hold_ms)
                }
                "cursor_trail_decay_ms" => {
                    config.cursor_trail.decay_ms =
                        parse_u64(words.next(), config.cursor_trail.decay_ms)
                }
                "cursor_trail_threshold" => {
                    config.cursor_trail.threshold =
                        parse_u16(words.next(), config.cursor_trail.threshold)
                }
                "cursor_trail_length" => {
                    config.cursor_trail.length =
                        parse_f32(words.next(), config.cursor_trail.length).clamp(0.05, 1.0)
                }
                "cursor_trail_color" => {
                    config.cursor_trail.color = parse_color(words.next(), config.cursor_trail.color)
                }
                _ => {}
            }
        }

        if config.plugins.is_empty() {
            config.plugins = Self::default().plugins;
        }
        config
    }
}

fn config_path() -> Option<PathBuf> {
    if let Ok(path) = env::var("C_TERM_CONFIG") {
        return Some(path.into());
    }
    if let Ok(path) = env::var("XDG_CONFIG_HOME") {
        return Some(PathBuf::from(path).join("c-term/config"));
    }
    env::var("HOME")
        .ok()
        .map(|home| PathBuf::from(home).join(".config/c-term/config"))
}

fn parse_u64(value: Option<&str>, fallback: u64) -> u64 {
    value
        .and_then(|value| value.parse().ok())
        .unwrap_or(fallback)
}

fn parse_u16(value: Option<&str>, fallback: u16) -> u16 {
    value
        .and_then(|value| value.parse().ok())
        .unwrap_or(fallback)
}

fn parse_f32(value: Option<&str>, fallback: f32) -> f32 {
    value
        .and_then(|value| value.parse().ok())
        .unwrap_or(fallback)
}

fn parse_color(value: Option<&str>, fallback: [u8; 3]) -> [u8; 3] {
    let Some(value) = value.and_then(|value| value.strip_prefix('#')) else {
        return fallback;
    };
    if value.len() != 6 {
        return fallback;
    }
    let parse = |range| u8::from_str_radix(&value[range], 16).ok();
    match (parse(0..2), parse(2..4), parse(4..6)) {
        (Some(r), Some(g), Some(b)) => [r, g, b],
        _ => fallback,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plugin_config() {
        let config = AppConfig::parse(
            "
            plugin cursor_trail
            cursor_trail_hold_ms 25
            cursor_trail_decay_ms 500
            cursor_trail_threshold 4
            cursor_trail_length 0.25
            cursor_trail_color #112233
            ",
        );

        assert_eq!(config.plugins, ["cursor_trail"]);
        assert_eq!(config.cursor_trail.hold_ms, 25);
        assert_eq!(config.cursor_trail.decay_ms, 500);
        assert_eq!(config.cursor_trail.threshold, 4);
        assert_eq!(config.cursor_trail.length, 0.25);
        assert_eq!(config.cursor_trail.color, [0x11, 0x22, 0x33]);
    }
}
