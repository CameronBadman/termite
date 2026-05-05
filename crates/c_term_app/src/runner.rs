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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ZoomConfig {
    pub(crate) default_steps: i16,
    pub(crate) persist: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct TextRenderConfig {
    pub(crate) text_weight: f32,
    pub(crate) symbol_weight: f32,
    pub(crate) text_gamma: f32,
    pub(crate) symbol_gamma: f32,
}

impl Default for TextRenderConfig {
    fn default() -> Self {
        Self {
            text_weight: 1.0,
            symbol_weight: 1.0,
            text_gamma: 1.0,
            symbol_gamma: 1.0,
        }
    }
}

impl TextRenderConfig {
    fn normalized(self) -> Self {
        Self {
            text_weight: self.text_weight.clamp(0.75, 1.35),
            symbol_weight: self.symbol_weight.clamp(0.75, 1.2),
            text_gamma: self.text_gamma.clamp(0.75, 1.25),
            symbol_gamma: self.symbol_gamma.clamp(0.75, 1.25),
        }
    }
}

impl Default for ZoomConfig {
    fn default() -> Self {
        Self {
            default_steps: 0,
            persist: true,
        }
    }
}

impl ZoomConfig {
    fn normalized(self) -> Self {
        Self {
            default_steps: self.default_steps.clamp(-4, 8),
            persist: self.persist,
        }
    }
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
    zoom: ZoomConfig,
    text_render: TextRenderConfig,
}

impl Runner {
    pub(crate) fn new() -> Self {
        Self {
            shell: env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_owned()),
            plugins: PluginHost::new(),
            font: FontConfig::default(),
            metrics: TerminalMetrics::default(),
            theme: Theme::default(),
            zoom: ZoomConfig::default(),
            text_render: TextRenderConfig::default(),
        }
    }

    pub(crate) fn with(mut self, part: impl RunnerPart) -> Self {
        part.install(&mut self);
        self
    }

    pub(crate) fn run(self) -> Result<(), Box<dyn Error>> {
        window_backend::run(self)
    }

    pub(crate) fn into_parts(
        self,
    ) -> (
        String,
        PluginHost,
        FontConfig,
        TerminalMetrics,
        Theme,
        ZoomConfig,
        TextRenderConfig,
    ) {
        (
            self.shell,
            self.plugins,
            self.font,
            self.metrics,
            self.theme,
            self.zoom,
            self.text_render,
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

    fn set_zoom(&mut self, zoom: ZoomConfig) {
        self.zoom = zoom.normalized();
    }

    fn set_text_render(&mut self, text_render: TextRenderConfig) {
        self.text_render = text_render.normalized();
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

    #[cfg(test)]
    pub(crate) fn zoom(&self) -> ZoomConfig {
        self.zoom
    }

    #[cfg(test)]
    pub(crate) fn text_render(&self) -> TextRenderConfig {
        self.text_render
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

pub(crate) fn terminal_zoom(zoom: ZoomConfig) -> ZoomPart {
    ZoomPart(zoom)
}

pub(crate) struct ZoomPart(ZoomConfig);

impl RunnerPart for ZoomPart {
    fn install(self, runner: &mut Runner) {
        runner.set_zoom(self.0);
    }
}

pub(crate) fn text_render(render: TextRenderConfig) -> TextRenderPart {
    TextRenderPart(render)
}

pub(crate) struct TextRenderPart(TextRenderConfig);

impl RunnerPart for TextRenderPart {
    fn install(self, runner: &mut Runner) {
        runner.set_text_render(self.0);
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
