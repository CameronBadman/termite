use std::{env, error::Error};

use crate::{plugins::PluginHost, theme::Theme, window_backend};

type InstallPart = Box<dyn FnOnce(&mut Runner)>;

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) enum FontConfig {
    #[default]
    Bitmap8x8,
    GlyphAtlas {
        path: String,
        size: f32,
    },
}

pub(crate) struct Runner {
    shell: String,
    plugins: PluginHost,
    font: FontConfig,
    theme: Theme,
}

impl Runner {
    pub(crate) fn new() -> Self {
        Self {
            shell: env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_owned()),
            plugins: PluginHost::new(),
            font: FontConfig::default(),
            theme: Theme::default(),
        }
    }

    pub(crate) fn with(mut self, part: impl RunnerPart) -> Self {
        part.install(&mut self);
        self
    }

    pub(crate) fn run(self) -> Result<(), Box<dyn Error>> {
        window_backend::run(self)
    }

    pub(crate) fn into_parts(self) -> (String, PluginHost, FontConfig, Theme) {
        (self.shell, self.plugins, self.font, self.theme)
    }

    fn add_plugin(&mut self, plugin: impl crate::plugins::Plugin + 'static) {
        self.plugins.add(plugin);
    }

    fn set_font(&mut self, font: FontConfig) {
        self.font = font;
    }

    fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
    }

    #[cfg(test)]
    pub(crate) fn plugin_count(&self) -> usize {
        self.plugins.len()
    }

    #[cfg(test)]
    pub(crate) fn font(&self) -> &FontConfig {
        &self.font
    }

    #[cfg(test)]
    pub(crate) fn theme(&self) -> Theme {
        self.theme
    }
}

pub(crate) trait RunnerPart {
    fn install(self, runner: &mut Runner);
}

impl<P> RunnerPart for P
where
    P: crate::plugins::Plugin + 'static,
{
    fn install(self, runner: &mut Runner) {
        runner.add_plugin(self);
    }
}

pub(crate) fn bitmap_font() -> FontPart {
    FontPart(FontConfig::Bitmap8x8)
}

pub(crate) fn font_file(path: impl Into<String>) -> FontPart {
    font_file_with_size(path, 14.0)
}

pub(crate) fn font_file_with_size(path: impl Into<String>, size: f32) -> FontPart {
    FontPart(FontConfig::GlyphAtlas {
        path: path.into(),
        size,
    })
}

pub(crate) struct FontPart(FontConfig);

impl RunnerPart for FontPart {
    fn install(self, runner: &mut Runner) {
        runner.set_font(self.0);
    }
}

pub(crate) fn theme(theme: Theme) -> ThemePart {
    ThemePart(theme)
}

pub(crate) struct ThemePart(Theme);

impl RunnerPart for ThemePart {
    fn install(self, runner: &mut Runner) {
        runner.set_theme(self.0);
    }
}

pub(crate) fn parts() -> Parts {
    Parts::new()
}

pub(crate) struct Parts {
    install: Vec<InstallPart>,
}

impl Parts {
    fn new() -> Self {
        Self {
            install: Vec::new(),
        }
    }

    pub(crate) fn with(mut self, part: impl RunnerPart + 'static) -> Self {
        self.install
            .push(Box::new(move |runner| part.install(runner)));
        self
    }
}

impl RunnerPart for Parts {
    fn install(self, runner: &mut Runner) {
        for install in self.install {
            install(runner);
        }
    }
}
