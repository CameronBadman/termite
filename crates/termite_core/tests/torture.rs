use termite_core::{Cell, Color, Grid, TerminalCore};

fn feed(terminal: &mut TerminalCore, input: impl AsRef<[u8]>) {
    let _ = terminal.process_pty_input(input.as_ref());
}

fn row_text(grid: &Grid, y: u16) -> String {
    (0..grid.width())
        .map(|x| grid.cell(x, y).unwrap().ch)
        .collect()
}

fn assert_screen(terminal: &TerminalCore, rows: &[&str]) {
    let grid = terminal.grid();
    assert_eq!(usize::from(grid.height()), rows.len());
    for (y, expected) in rows.iter().enumerate() {
        assert_eq!(row_text(grid, y as u16), *expected, "row {y}");
    }
}

fn cell(grid: &Grid, x: u16, y: u16) -> Cell {
    grid.cell(x, y).unwrap()
}

fn assert_wide_invariants(grid: &Grid) {
    for y in 0..grid.height() {
        for x in 0..grid.width() {
            let current = cell(grid, x, y);
            if current.wide {
                assert!(
                    x + 1 < grid.width(),
                    "wide cell at row {y} col {x} has no spacer slot"
                );
                assert!(
                    cell(grid, x + 1, y).spacer,
                    "wide cell at row {y} col {x} has no spacer"
                );
            }
            if current.spacer {
                assert!(x > 0, "orphan spacer at row {y} col {x}");
                assert!(
                    cell(grid, x - 1, y).wide,
                    "orphan spacer at row {y} col {x}"
                );
            }
        }
    }
}

#[test]
fn tmux_style_pane_border_survives_exact_left_pane_write() {
    let mut terminal = TerminalCore::new(10, 3);
    feed(
        &mut terminal,
        b"\x1b[1;5H|\x1b[2;5H|\x1b[3;5H|\
          \x1b[1;1Habcd\
          \x1b[2;1Hxy",
    );

    assert_screen(&terminal, &["abcd|     ", "xy  |     ", "    |     "]);
    assert_eq!(cell(terminal.grid(), 4, 0).ch, '|');
    assert_eq!(cell(terminal.grid(), 4, 1).ch, '|');
}

#[test]
fn tmux_style_right_pane_wrap_does_not_touch_left_pane() {
    let mut terminal = TerminalCore::new(10, 3);
    feed(
        &mut terminal,
        b"\x1b[1;5H|\x1b[2;5H|\x1b[3;5H|\
          \x1b[1;1Hleft\
          \x1b[1;6H\x1b[?7labcdef\x1b[?7h",
    );

    assert_screen(&terminal, &["left|abcdf", "    |     ", "    |     "]);
}

#[test]
fn pending_wrap_is_cancelled_by_cursor_motion() {
    let mut terminal = TerminalCore::new(4, 2);
    feed(&mut terminal, b"abcd\x1b[1;1HZ");

    assert_screen(&terminal, &["Zbcd", "    "]);
    assert_eq!(terminal.grid().cursor().x, 1);
    assert_eq!(terminal.grid().cursor().y, 0);
}

#[test]
fn pending_wrap_is_cancelled_by_carriage_return() {
    let mut terminal = TerminalCore::new(4, 2);
    feed(&mut terminal, b"abcd\rZ");

    assert_screen(&terminal, &["Zbcd", "    "]);
    assert_eq!(terminal.grid().cursor().x, 1);
    assert_eq!(terminal.grid().cursor().y, 0);
}

#[test]
fn index_escape_preserves_column_and_scrolls_at_bottom_margin() {
    let mut terminal = TerminalCore::new(4, 3);
    feed(&mut terminal, b"ab\x1bDcd\x1b[3;3H\x1bDZZ");

    assert_screen(&terminal, &["  cd", "    ", "  ZZ"]);
    assert_eq!(terminal.grid().cursor().x, 3);
    assert_eq!(terminal.grid().cursor().y, 2);
}

#[test]
fn next_line_and_reverse_index_match_vt_cursor_controls() {
    let mut terminal = TerminalCore::new(4, 3);
    feed(&mut terminal, b"ab\x1bEcd\x1b[1;1H\x1bMZ");

    assert_screen(&terminal, &["Z   ", "ab  ", "cd  "]);
    assert_eq!(terminal.grid().cursor().x, 1);
    assert_eq!(terminal.grid().cursor().y, 0);
}

#[test]
fn screen_alignment_fills_visible_grid_and_clears_wide_state() {
    let mut terminal = TerminalCore::new(4, 2);
    feed(&mut terminal, "表x\x1b[2;3Hy\x1b#8".as_bytes());

    assert_screen(&terminal, &["EEEE", "EEEE"]);
    assert_wide_invariants(terminal.grid());
    for y in 0..terminal.grid().height() {
        for x in 0..terminal.grid().width() {
            let current = cell(terminal.grid(), x, y);
            assert!(!current.wide);
            assert!(!current.spacer);
        }
    }
}

#[test]
fn ignored_c0_controls_do_not_split_or_pollute_printed_text() {
    let mut terminal = TerminalCore::new(5, 1);
    feed(&mut terminal, b"A\x00\x01\x02B\x06C");

    assert_screen(&terminal, &["ABC  "]);
}

#[test]
fn osc_window_title_is_ignored_without_polluting_grid() {
    let mut terminal = TerminalCore::new(4, 1);
    feed(&mut terminal, b"\x1b]0;title\x07AB");

    assert_screen(&terminal, &["AB  "]);
}

#[test]
fn osc8_hyperlink_wrappers_do_not_print_control_payload() {
    let mut terminal = TerminalCore::new(4, 1);
    feed(
        &mut terminal,
        b"A\x1b]8;;https://example.com\x1b\\B\x1b]8;;\x1b\\C",
    );

    assert_screen(&terminal, &["ABC "]);
}

#[test]
fn horizontal_tab_advances_to_next_eight_column_stop() {
    let mut terminal = TerminalCore::new(10, 1);
    feed(&mut terminal, b"A\tB");

    assert_screen(&terminal, &["A       B "]);
    assert_eq!(terminal.grid().cursor().x, 9);
}

#[test]
fn csi_leading_zeroes_and_empty_params_use_vt_defaults() {
    let mut terminal = TerminalCore::new(4, 3);
    feed(&mut terminal, b"\x1b[0002;0003HZ\x1b[;HY");

    assert_screen(&terminal, &["Y   ", "  Z ", "    "]);
}

#[test]
fn cursor_position_is_clamped_to_visible_grid() {
    let mut terminal = TerminalCore::new(4, 3);
    feed(&mut terminal, b"\x1b[999;999HZ");

    assert_screen(&terminal, &["    ", "    ", "   Z"]);
    assert_eq!(terminal.grid().cursor().x, 3);
    assert_eq!(terminal.grid().cursor().y, 2);
}

#[test]
fn repeat_with_default_count_reprints_once() {
    let mut terminal = TerminalCore::new(5, 1);
    feed(&mut terminal, b"A\x1b[b");

    assert_screen(&terminal, &["AA   "]);
}

#[test]
fn sgr_256_colors_apply_to_foreground_and_background() {
    let mut terminal = TerminalCore::new(4, 1);
    feed(&mut terminal, b"\x1b[38;5;196mR\x1b[48;5;46mG\x1b[0mX");

    assert_screen(&terminal, &["RGX "]);
    assert_eq!(
        cell(terminal.grid(), 0, 0).style.foreground,
        Color::Indexed(196)
    );
    assert_eq!(
        cell(terminal.grid(), 1, 0).style.background,
        Color::Indexed(46)
    );
    assert_eq!(
        cell(terminal.grid(), 2, 0).style.foreground,
        Color::DefaultForeground
    );
    assert_eq!(
        cell(terminal.grid(), 2, 0).style.background,
        Color::DefaultBackground
    );
}

#[test]
fn sgr_truecolor_semicolon_and_colon_forms_apply() {
    let mut terminal = TerminalCore::new(3, 1);
    feed(&mut terminal, b"\x1b[38;2;1;2;3mA\x1b[48:2:4:5:6mB");

    assert_screen(&terminal, &["AB "]);
    assert_eq!(
        cell(terminal.grid(), 0, 0).style.foreground,
        Color::Rgb(1, 2, 3)
    );
    assert_eq!(
        cell(terminal.grid(), 1, 0).style.background,
        Color::Rgb(4, 5, 6)
    );
}

#[test]
fn sgr_bright_colors_and_selective_resets_apply() {
    let mut terminal = TerminalCore::new(4, 1);
    feed(&mut terminal, b"\x1b[91;104mA\x1b[39mB\x1b[49mC");

    assert_screen(&terminal, &["ABC "]);
    assert_eq!(
        cell(terminal.grid(), 0, 0).style.foreground,
        Color::Indexed(9)
    );
    assert_eq!(
        cell(terminal.grid(), 0, 0).style.background,
        Color::Indexed(12)
    );
    assert_eq!(
        cell(terminal.grid(), 1, 0).style.foreground,
        Color::DefaultForeground
    );
    assert_eq!(
        cell(terminal.grid(), 1, 0).style.background,
        Color::Indexed(12)
    );
    assert_eq!(
        cell(terminal.grid(), 2, 0).style.background,
        Color::DefaultBackground
    );
}

#[test]
fn wide_character_at_right_edge_wraps_without_corrupting_edge_cell() {
    let mut terminal = TerminalCore::new(4, 2);
    feed(&mut terminal, "abc表Z".as_bytes());

    assert_screen(&terminal, &["abc ", "表 Z "]);
    assert_eq!(cell(terminal.grid(), 0, 1).ch, '表');
    assert!(cell(terminal.grid(), 0, 1).wide);
    assert!(cell(terminal.grid(), 1, 1).spacer);
    assert_wide_invariants(terminal.grid());
}

#[test]
fn overwriting_wide_lead_clears_trailing_spacer() {
    let mut terminal = TerminalCore::new(4, 1);
    feed(&mut terminal, "表\x1b[1;1HX".as_bytes());

    assert_screen(&terminal, &["X   "]);
    assert!(!cell(terminal.grid(), 0, 0).wide);
    assert!(!cell(terminal.grid(), 1, 0).spacer);
    assert_wide_invariants(terminal.grid());
}

#[test]
fn erase_from_wide_spacer_clears_whole_wide_character() {
    let mut terminal = TerminalCore::new(4, 1);
    feed(&mut terminal, "表X\x1b[1;2H\x1b[K".as_bytes());

    assert_screen(&terminal, &["    "]);
    assert!(!cell(terminal.grid(), 0, 0).wide);
    assert!(!cell(terminal.grid(), 1, 0).spacer);
    assert_wide_invariants(terminal.grid());
}

#[test]
fn erase_chars_from_wide_spacer_clears_whole_wide_character() {
    let mut terminal = TerminalCore::new(5, 1);
    feed(&mut terminal, "A表B\x1b[1;3H\x1b[1X".as_bytes());

    assert_screen(&terminal, &["A  B "]);
    assert!(!cell(terminal.grid(), 1, 0).wide);
    assert!(!cell(terminal.grid(), 2, 0).spacer);
    assert_wide_invariants(terminal.grid());
}

#[test]
fn insert_blank_chars_near_wide_character_keeps_row_sane() {
    let mut terminal = TerminalCore::new(6, 1);
    feed(&mut terminal, "A表BC\x1b[1;2H\x1b[1@".as_bytes());

    assert_screen(&terminal, &["A 表 BC"]);
    assert!(cell(terminal.grid(), 2, 0).wide);
    assert!(cell(terminal.grid(), 3, 0).spacer);
    assert_wide_invariants(terminal.grid());
}

#[test]
fn delete_chars_near_wide_character_keeps_row_sane() {
    let mut terminal = TerminalCore::new(6, 1);
    feed(&mut terminal, "A表BCD\x1b[1;2H\x1b[1P".as_bytes());

    assert_screen(&terminal, &["A BCD "]);
    assert_wide_invariants(terminal.grid());
}

#[test]
fn scroll_region_keeps_pane_header_and_footer_stable() {
    let mut terminal = TerminalCore::new(6, 5);
    feed(
        &mut terminal,
        b"header\x1b[2;1Hrow-1 \x1b[3;1Hrow-2 \x1b[4;1Hrow-3 \x1b[5;1Hfooter\
          \x1b[2;4r\x1b[4;1H\nnew!!",
    );

    assert_screen(
        &terminal,
        &["header", "row-2 ", "row-3 ", "new!! ", "footer"],
    );
}

#[test]
fn insert_and_delete_lines_are_limited_to_scroll_region() {
    let mut terminal = TerminalCore::new(5, 5);
    feed(
        &mut terminal,
        b"top__\x1b[2;1Hone__\x1b[3;1Htwo__\x1b[4;1Hthree\x1b[5;1Hbot__\
          \x1b[2;4r\x1b[3;1H\x1b[1L",
    );

    assert_screen(&terminal, &["top__", "one__", "     ", "two__", "bot__"]);

    feed(&mut terminal, b"\x1b[2;1H\x1b[1M");
    assert_screen(&terminal, &["top__", "     ", "two__", "     ", "bot__"]);
}

#[test]
fn alternate_screen_preserves_primary_scrollback_and_content() {
    let mut terminal = TerminalCore::new(4, 2);
    feed(&mut terminal, b"aa\r\nbb\r\ncc");
    assert_eq!(terminal.scrollback_len(), 1);

    feed(&mut terminal, b"\x1b[?1049hXX\r\nYY\r\nZZ\x1b[?1049l");

    assert_eq!(terminal.scrollback_len(), 1);
    assert_eq!(row_text(terminal.grid(), 0), "bb  ");
    assert_eq!(row_text(terminal.grid(), 1), "cc  ");
}

#[test]
fn resize_reflow_preserves_wide_character_shape() {
    let mut terminal = TerminalCore::new(6, 2);
    feed(&mut terminal, "ab表cd".as_bytes());

    let _ = terminal.resize_reflow(4, 3);

    assert_screen(&terminal, &["ab表 ", "cd  ", "    "]);
    assert!(cell(terminal.grid(), 2, 0).wide);
    assert!(cell(terminal.grid(), 3, 0).spacer);
    assert_wide_invariants(terminal.grid());
}
