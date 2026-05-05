use std::{
    hint::black_box,
    time::{Duration, Instant},
};

use c_term_core::TerminalCore;

const COLS: u16 = 100;
const ROWS: u16 = 40;
const CHUNK: usize = 8192;

fn main() {
    let workloads = [
        workload_plain_scroll(),
        workload_sgr_table(),
        workload_cursor_moves(),
        workload_unicode(),
    ];

    println!(
        "{:<18} {:>9} {:>10} {:>10} {:>10} {:>10}",
        "workload", "bytes", "elapsed", "MiB/s", "ns/byte", "damage"
    );
    for workload in workloads {
        let result = run_workload(&workload);
        println!(
            "{:<18} {:>9} {:>10.2} {:>10.2} {:>10.1} {:>10}",
            workload.name,
            workload.bytes.len(),
            result.elapsed.as_secs_f64() * 1000.0,
            result.mib_per_second(),
            result.ns_per_byte(),
            result.damage_regions,
        );
    }
}

struct Workload {
    name: &'static str,
    bytes: Vec<u8>,
}

struct RunResult {
    elapsed: Duration,
    bytes: usize,
    damage_regions: usize,
}

impl RunResult {
    fn mib_per_second(&self) -> f64 {
        let mib = self.bytes as f64 / (1024.0 * 1024.0);
        mib / self.elapsed.as_secs_f64()
    }

    fn ns_per_byte(&self) -> f64 {
        self.elapsed.as_secs_f64() * 1_000_000_000.0 / self.bytes.max(1) as f64
    }
}

fn run_workload(workload: &Workload) -> RunResult {
    let mut terminal = TerminalCore::new(COLS, ROWS);
    let started = Instant::now();
    let mut damage_regions = 0;

    for chunk in workload.bytes.chunks(CHUNK) {
        let tick = terminal.process_pty_input(chunk);
        damage_regions += tick.damage.regions.len();
        black_box(tick.output.len());
        black_box(tick.clipboard.len());
    }
    black_box(terminal.grid().generation());
    black_box(terminal.scrollback_len());

    RunResult {
        elapsed: started.elapsed(),
        bytes: workload.bytes.len(),
        damage_regions,
    }
}

fn workload_plain_scroll() -> Workload {
    let mut bytes = Vec::new();
    for i in 0..60_000 {
        bytes.extend_from_slice(
            format!("plain line {i:05} abcdefghijklmnopqrstuvwxyz\r\n").as_bytes(),
        );
    }
    Workload {
        name: "plain-scroll",
        bytes,
    }
}

fn workload_sgr_table() -> Workload {
    let mut bytes = Vec::new();
    for row in 0..20_000 {
        for color in 0..16 {
            bytes.extend_from_slice(
                format!("\x1b[{}m{:02x} ", 30 + color % 8, row + color).as_bytes(),
            );
        }
        bytes.extend_from_slice(b"\x1b[0m\r\n");
    }
    Workload {
        name: "sgr-table",
        bytes,
    }
}

fn workload_cursor_moves() -> Workload {
    let mut bytes = Vec::new();
    for i in 0..80_000 {
        let x = i % usize::from(COLS) + 1;
        let y = i % usize::from(ROWS) + 1;
        bytes.extend_from_slice(format!("\x1b[{y};{x}H{i:08x}").as_bytes());
    }
    Workload {
        name: "cursor-moves",
        bytes,
    }
}

fn workload_unicode() -> Workload {
    let mut bytes = Vec::new();
    let sample = "unicode 表 λ π ┌─┐ █ ░ we’ll — ok\r\n";
    for _ in 0..50_000 {
        bytes.extend_from_slice(sample.as_bytes());
    }
    Workload {
        name: "unicode",
        bytes,
    }
}
