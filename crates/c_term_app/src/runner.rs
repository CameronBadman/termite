use std::{env, error::Error};

use crate::{plugins::PluginHost, theme::Theme, window_backend};

type InstallPart = Box<dyn FnOnce(&mut Runner)>;

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) enum FontConfig {
    #[default]
    Bitmap8x8,
    GlyphAtlas {
        paths: Vec<String>,
        size: f32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TerminalMetrics {
    pub(crate) cell_width: u32,
    pub(crate) cell_height: u32,
}

impl Default for TerminalMetrics {
    fn default() -> Self {
        Self {
            cell_width: 10,
            cell_height: 18,
        }
    }
}

impl TerminalMetrics {
    fn normalized(self) -> Self {
        Self {
            cell_width: self.cell_width.max(1),
            cell_height: self.cell_height.max(1),
        }
    }
}

pub(crate) struct Runner {
    shell: String,
    plugins: PluginHost,
    font: FontConfig,
    metrics: TerminalMetrics,
    theme: Theme,
}

impl Runner {
    pub(crate) fn new() -> Self {
        Self {
            shell: env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_owned()),
            plugins: PluginHost::new(),
            font: FontConfig::default(),
            metrics: TerminalMetrics::default(),
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

    pub(crate) fn into_parts(self) -> (String, PluginHost, FontConfig, TerminalMetrics, Theme) {
        (
            self.shell,
            self.plugins,
            self.font,
            self.metrics,
            self.theme,
        )
    }

    fn add_plugin(&mut self, plugin: impl crate::plugins::Plugin + 'static) {
        self.plugins.add(plugin);
    }

    fn set_font(&mut self, font: FontConfig) {
        self.font = font;
    }

    fn set_metrics(&mut self, metrics: TerminalMetrics) {
        self.metrics = metrics.normalized();
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
    pub(crate) fn metrics(&self) -> TerminalMetrics {
        self.metrics
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

#[allow(dead_code)]
pub(crate) fn font_file(path: impl Into<String>) -> FontPart {
    font_file_with_size(path, 14.0)
}

#[allow(dead_code)]
pub(crate) fn font_file_with_size(path: impl Into<String>, size: f32) -> FontPart {
    font_files_with_size([path], size)
}

#[allow(dead_code)]
pub(crate) fn font_files(paths: impl IntoIterator<Item = impl Into<String>>) -> FontPart {
    font_files_with_size(paths, 14.0)
}

pub(crate) fn font_files_with_size(
    paths: impl IntoIterator<Item = impl Into<String>>,
    size: f32,
) -> FontPart {
    FontPart(FontConfig::GlyphAtlas {
        paths: paths.into_iter().map(Into::into).collect(),
        size,
    })
}

pub(crate) struct FontPart(FontConfig);

impl RunnerPart for FontPart {
    fn install(self, runner: &mut Runner) {
        runner.set_font(self.0);
    }
}

pub(crate) fn terminal_metrics(metrics: TerminalMetrics) -> MetricsPart {
    MetricsPart(metrics)
}

pub(crate) struct MetricsPart(TerminalMetrics);

impl RunnerPart for MetricsPart {
    fn install(self, runner: &mut Runner) {
        runner.set_metrics(self.0);
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
