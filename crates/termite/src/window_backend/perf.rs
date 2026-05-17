use std::{
    env,
    time::{Duration, Instant},
};

use termite_core::CoreProfile;

#[derive(Debug, Clone)]
pub(super) struct PerfStats {
    pub(super) enabled: bool,
    interval_start: Instant,
    pty_events: u64,
    pty_bytes: u64,
    pty_core_time: Duration,
    core_fast_sgr_time: Duration,
    core_fast_text_time: Duration,
    core_parser_time: Duration,
    core_apply_time: Duration,
    core_tick_time: Duration,
    core_fast_sgr_calls: u64,
    core_fast_text_calls: u64,
    core_parser_bytes: u64,
    core_actions: u64,
    damage_regions: u64,
    frames: u64,
    full_uploads: u64,
    row_bands: u64,
    scroll_uploads: u64,
    overlays: u64,
    cache_time: Duration,
    plugin_time: Duration,
    gpu_time: Duration,
    render_time: Duration,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct PerfFrameUpdate {
    pub(super) upload_full: bool,
    pub(super) upload_row_bands: usize,
    pub(super) upload_scrolls: usize,
    pub(super) overlays: usize,
}

impl PerfStats {
    pub(super) fn from_env() -> Self {
        Self {
            enabled: env::var_os("TERMITE_PERF").is_some(),
            interval_start: Instant::now(),
            pty_events: 0,
            pty_bytes: 0,
            pty_core_time: Duration::ZERO,
            core_fast_sgr_time: Duration::ZERO,
            core_fast_text_time: Duration::ZERO,
            core_parser_time: Duration::ZERO,
            core_apply_time: Duration::ZERO,
            core_tick_time: Duration::ZERO,
            core_fast_sgr_calls: 0,
            core_fast_text_calls: 0,
            core_parser_bytes: 0,
            core_actions: 0,
            damage_regions: 0,
            frames: 0,
            full_uploads: 0,
            row_bands: 0,
            scroll_uploads: 0,
            overlays: 0,
            cache_time: Duration::ZERO,
            plugin_time: Duration::ZERO,
            gpu_time: Duration::ZERO,
            render_time: Duration::ZERO,
        }
    }

    pub(super) fn record_pty(
        &mut self,
        bytes: usize,
        damage_regions: usize,
        core_time: Duration,
        core_profile: CoreProfile,
    ) {
        if !self.enabled {
            return;
        }
        self.pty_events += 1;
        self.pty_bytes += bytes as u64;
        self.damage_regions += damage_regions as u64;
        self.pty_core_time += core_time;
        self.core_fast_sgr_time += core_profile.fast_sgr_time;
        self.core_fast_text_time += core_profile.fast_text_time;
        self.core_parser_time += core_profile.parser_time;
        self.core_apply_time += core_profile.apply_time;
        self.core_tick_time += core_profile.tick_time;
        self.core_fast_sgr_calls += core_profile.fast_sgr_calls;
        self.core_fast_text_calls += core_profile.fast_text_calls;
        self.core_parser_bytes += core_profile.parser_bytes;
        self.core_actions += core_profile.actions;
        self.report_if_due();
    }

    pub(super) fn record_frame(
        &mut self,
        cache_time: Duration,
        plugin_time: Duration,
        gpu_time: Duration,
        render_time: Duration,
        update: PerfFrameUpdate,
    ) {
        if !self.enabled {
            return;
        }
        self.frames += 1;
        self.full_uploads += u64::from(update.upload_full);
        self.row_bands += update.upload_row_bands as u64;
        self.scroll_uploads += update.upload_scrolls as u64;
        self.overlays += update.overlays as u64;
        self.cache_time += cache_time;
        self.plugin_time += plugin_time;
        self.gpu_time += gpu_time;
        self.render_time += render_time;
        self.report_if_due();
    }

    fn report_if_due(&mut self) {
        let elapsed = self.interval_start.elapsed();
        if elapsed < Duration::from_secs(1) {
            return;
        }
        self.report_elapsed(elapsed);
    }

    pub(super) fn report_final(&mut self) {
        if !self.enabled {
            return;
        }
        if self.pty_events == 0 && self.frames == 0 {
            return;
        }
        self.report_elapsed(self.interval_start.elapsed());
    }

    fn report_elapsed(&mut self, elapsed: Duration) {
        if elapsed.is_zero() {
            return;
        }

        let seconds = elapsed.as_secs_f64();
        let mib = self.pty_bytes as f64 / (1024.0 * 1024.0);
        eprintln!(
            concat!(
                "termite-perf ",
                "pty={:.2}MiB/s events={} damage={} ",
                "frames={} full={} rows={} scrolls={} overlays={} ",
                "core={:.2}ms fast_sgr={:.2}ms/{} fast_text={:.2}ms/{} ",
                "parse={:.2}ms/{}B apply={:.2}ms/{} tick={:.2}ms ",
                "cache={:.2}ms plugins={:.2}ms gpu={:.2}ms render={:.2}ms"
            ),
            mib / seconds,
            self.pty_events,
            self.damage_regions,
            self.frames,
            self.full_uploads,
            self.row_bands,
            self.scroll_uploads,
            self.overlays,
            duration_ms(self.pty_core_time),
            duration_ms(self.core_fast_sgr_time),
            self.core_fast_sgr_calls,
            duration_ms(self.core_fast_text_time),
            self.core_fast_text_calls,
            duration_ms(self.core_parser_time),
            self.core_parser_bytes,
            duration_ms(self.core_apply_time),
            self.core_actions,
            duration_ms(self.core_tick_time),
            duration_ms(self.cache_time),
            duration_ms(self.plugin_time),
            duration_ms(self.gpu_time),
            duration_ms(self.render_time),
        );
        *self = Self {
            enabled: true,
            ..Self::from_env()
        };
    }
}

pub(super) fn duration_ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}
