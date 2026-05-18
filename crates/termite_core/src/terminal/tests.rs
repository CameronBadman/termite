use super::*;

#[test]
fn empty_input_is_idle_after_initial_damage_drained() {
    let mut terminal = TerminalCore::new(2, 2);
    let _ = terminal.process_pty_input(b"");

    assert!(terminal.process_pty_input(b"").is_idle());
}

#[test]
fn printable_input_changes_cells_and_cursor() {
    let mut terminal = TerminalCore::new(3, 2);
    let tick = terminal.process_pty_input(b"ab");

    assert_eq!(terminal.grid().cell(0, 0).unwrap().ch, 'a');
    assert_eq!(terminal.grid().cell(1, 0).unwrap().ch, 'b');
    assert!(!tick.damage.is_empty());
}

#[test]
fn fast_ascii_path_handles_plain_controls() {
    let mut terminal = TerminalCore::new(4, 2);
    let _ = terminal.process_pty_input(b"ab\rc\nD");

    assert_eq!(row_text(terminal.grid(), 0), "cb  ");
    assert_eq!(row_text(terminal.grid(), 1), " D  ");
}

#[test]
fn fast_text_path_handles_utf8_and_plain_controls() {
    let mut terminal = TerminalCore::new(6, 2);
    let _ = terminal.process_pty_input("abλπ\rc\nok".as_bytes());

    assert_eq!(row_text(terminal.grid(), 0), "cbλπ  ");
    assert_eq!(row_text(terminal.grid(), 1), " ok   ");
}

#[test]
fn fast_text_path_wraps_width_one_utf8_like_scalar_writes() {
    let mut terminal = TerminalCore::new(4, 2);
    let _ = terminal.process_pty_input("λπ┌─┐".as_bytes());

    assert_eq!(row_text(terminal.grid(), 0), "λπ┌─");
    assert_eq!(row_text(terminal.grid(), 1), "┐   ");
}

#[test]
fn fast_sgr_path_resumes_fast_text_after_color_sequence() {
    let mut terminal = TerminalCore::new(5, 1);
    let _ = terminal.process_pty_input(b"\x1b[31mred\x1b[0m!");

    assert_eq!(row_text(terminal.grid(), 0), "red! ");
    assert_eq!(
        terminal.grid().cell(0, 0).unwrap().style.foreground,
        crate::Color::Indexed(1)
    );
    assert_eq!(
        terminal.grid().cell(3, 0).unwrap().style.foreground,
        crate::Color::DefaultForeground
    );
}

#[test]
fn clear_command_sequence_clears_visible_screen() {
    let mut terminal = TerminalCore::new(4, 2);
    let tick = terminal.process_pty_input(b"ab\x1b[2;1Hcd\x1b[H\x1b[J");

    assert_eq!(row_text(terminal.grid(), 0), "    ");
    assert_eq!(row_text(terminal.grid(), 1), "    ");
    assert!(!tick.damage.is_empty());
}

#[test]
fn full_clear_sequence_clears_visible_screen() {
    let mut terminal = TerminalCore::new(4, 2);
    let _ = terminal.process_pty_input(b"ab\x1b[2;1Hcd\x1b[H\x1b[2J\x1b[3J");

    assert_eq!(row_text(terminal.grid(), 0), "    ");
    assert_eq!(row_text(terminal.grid(), 1), "    ");
}

#[test]
fn clear_scrollback_sequence_removes_history() {
    let mut terminal = TerminalCore::new(3, 2);
    let _ = terminal.process_pty_input(b"ab\r\ncd\r\nef");
    assert_eq!(terminal.scrollback_len(), 1);

    let _ = terminal.process_pty_input(b"\x1b[3J");

    assert_eq!(terminal.scrollback_len(), 0);
}

#[test]
fn reset_sequence_resets_screen_and_cursor() {
    let mut terminal = TerminalCore::new(4, 2);
    let _ = terminal.process_pty_input(b"ab\x1b[2;3Hcd\x1bc");

    assert_eq!(row_text(terminal.grid(), 0), "    ");
    assert_eq!(row_text(terminal.grid(), 1), "    ");
    assert_eq!(terminal.grid().cursor().x, 0);
    assert_eq!(terminal.grid().cursor().y, 0);
}

#[test]
fn resize_emits_viewport_damage() {
    let mut terminal = TerminalCore::new(3, 2);
    let _ = terminal.process_pty_input(b"");
    let tick = terminal.resize(4, 4);

    assert_eq!(terminal.grid().width(), 4);
    assert!(
        tick.damage
            .regions
            .iter()
            .any(|region| matches!(region, crate::DamageRegion::Viewport))
    );
}

#[test]
fn resize_preserves_cell_coordinates() {
    let mut terminal = TerminalCore::new(4, 2);
    let _ = terminal.process_pty_input(b"ab\x1b[2;1Hcd");

    let _ = terminal.resize(6, 3);

    assert_eq!(terminal.grid().cell(0, 0).unwrap().ch, 'a');
    assert_eq!(terminal.grid().cell(1, 0).unwrap().ch, 'b');
    assert_eq!(terminal.grid().cell(0, 1).unwrap().ch, 'c');
    assert_eq!(terminal.grid().cell(1, 1).unwrap().ch, 'd');
}

#[test]
fn resize_truncates_rows_by_coordinates() {
    let mut terminal = TerminalCore::new(5, 2);
    let _ = terminal.process_pty_input(b"abcd\x1b[2;1Hefgh");

    let _ = terminal.resize(2, 2);

    assert_eq!(terminal.grid().cell(0, 0).unwrap().ch, 'a');
    assert_eq!(terminal.grid().cell(1, 0).unwrap().ch, 'b');
    assert_eq!(terminal.grid().cell(0, 1).unwrap().ch, 'e');
    assert_eq!(terminal.grid().cell(1, 1).unwrap().ch, 'f');
}

#[test]
fn resize_reflow_wraps_existing_rows_to_new_width() {
    let mut terminal = TerminalCore::new(6, 3);
    let _ = terminal.process_pty_input(b"abcdef");

    let _ = terminal.resize_reflow(3, 3);

    assert_eq!(row_text(terminal.grid(), 0), "abc");
    assert_eq!(row_text(terminal.grid(), 1), "def");
    assert_eq!(row_text(terminal.grid(), 2), "   ");
}

#[test]
fn resize_reflow_moves_overflow_into_scrollback() {
    let mut terminal = TerminalCore::new(6, 2);
    let _ = terminal.process_pty_input(b"abcdef\x1b[2;1Hghijkl");

    let _ = terminal.resize_reflow(3, 2);

    assert_eq!(terminal.scrollback_len(), 2);
    assert_eq!(row_slice_text(terminal.scrollback_row(0).unwrap()), "abc");
    assert_eq!(row_slice_text(terminal.scrollback_row(1).unwrap()), "def");
    assert_eq!(row_text(terminal.grid(), 0), "ghi");
    assert_eq!(row_text(terminal.grid(), 1), "jkl");
}

#[test]
fn erase_chars_removes_stale_cells() {
    let mut terminal = TerminalCore::new(6, 1);
    let _ = terminal.process_pty_input(b"abcde\x1b[1;2H\x1b[2X");

    assert_eq!(terminal.grid().cell(0, 0).unwrap().ch, 'a');
    assert_eq!(terminal.grid().cell(1, 0).unwrap().ch, ' ');
    assert_eq!(terminal.grid().cell(2, 0).unwrap().ch, ' ');
    assert_eq!(terminal.grid().cell(3, 0).unwrap().ch, 'd');
}

#[test]
fn delete_chars_shifts_line_left() {
    let mut terminal = TerminalCore::new(7, 1);
    let _ = terminal.process_pty_input(b"abcdef\x1b[1;3H\x1b[2P");

    assert_eq!(row_text(terminal.grid(), 0), "abef   ");
}

#[test]
fn insert_blank_chars_shifts_line_right() {
    let mut terminal = TerminalCore::new(7, 1);
    let _ = terminal.process_pty_input(b"abcdef\x1b[1;3H\x1b[2@");

    assert_eq!(row_text(terminal.grid(), 0), "ab  cde");
}

#[test]
fn delete_and_insert_lines_clear_scrolled_rows() {
    let mut terminal = TerminalCore::new(4, 3);
    let _ = terminal.process_pty_input(b"aaa\x1b[2;1Hbbb\x1b[3;1Hccc\x1b[2;1H\x1b[1M");

    assert_eq!(row_text(terminal.grid(), 0), "aaa ");
    assert_eq!(row_text(terminal.grid(), 1), "ccc ");
    assert_eq!(row_text(terminal.grid(), 2), "    ");

    let _ = terminal.process_pty_input(b"\x1b[2;1H\x1b[1L");
    assert_eq!(row_text(terminal.grid(), 1), "    ");
    assert_eq!(row_text(terminal.grid(), 2), "ccc ");
}

#[test]
fn scroll_up_and_down_apply_to_scroll_region() {
    let mut terminal = TerminalCore::new(4, 4);
    let _ = terminal
        .process_pty_input(b"aaa\x1b[2;1Hbbb\x1b[3;1Hccc\x1b[4;1Hddd\x1b[2;4r\x1b[1;1H\x1b[1S");

    assert_eq!(row_text(terminal.grid(), 0), "aaa ");
    assert_eq!(row_text(terminal.grid(), 1), "ccc ");
    assert_eq!(row_text(terminal.grid(), 2), "ddd ");
    assert_eq!(row_text(terminal.grid(), 3), "    ");

    let _ = terminal.process_pty_input(b"\x1b[1T");
    assert_eq!(row_text(terminal.grid(), 1), "    ");
    assert_eq!(row_text(terminal.grid(), 2), "ccc ");
    assert_eq!(row_text(terminal.grid(), 3), "ddd ");
}

#[test]
fn csi_cursor_position_writes_at_requested_cell() {
    let mut terminal = TerminalCore::new(4, 3);
    let _ = terminal.process_pty_input(b"\x1b[2;3Hx");

    assert_eq!(terminal.grid().cell(2, 1).unwrap().ch, 'x');
}

#[test]
fn line_feed_scrolls_at_bottom() {
    let mut terminal = TerminalCore::new(3, 2);
    let _ = terminal.process_pty_input(b"ab\r\ncd\r\nef");

    assert_eq!(terminal.grid().cell(0, 0).unwrap().ch, 'c');
    assert_eq!(terminal.grid().cell(1, 0).unwrap().ch, 'd');
    assert_eq!(terminal.grid().cell(0, 1).unwrap().ch, 'e');
    assert_eq!(terminal.grid().cell(1, 1).unwrap().ch, 'f');
}

#[test]
fn primary_full_screen_scroll_adds_scrollback() {
    let mut terminal = TerminalCore::new(3, 2);
    let _ = terminal.process_pty_input(b"ab\r\ncd\r\nef");

    assert_eq!(terminal.scrollback_len(), 1);
    assert_eq!(row_slice_text(terminal.scrollback_row(0).unwrap()), "ab");
}

#[test]
fn scrollback_trims_rows_overwritten_with_default_spaces() {
    let mut terminal = TerminalCore::new(5, 2);
    let _ = terminal.process_pty_input(b"abc\r   \r\nx\r\n");

    assert_eq!(terminal.scrollback_len(), 1);
    assert_eq!(row_slice_text(terminal.scrollback_row(0).unwrap()), "");
}

#[test]
fn scrollback_append_keeps_only_capacity_tail() {
    let mut terminal = TerminalCore::new(3, 1);
    terminal.scrollback_capacity = 2;
    let _ = terminal.process_pty_input(b"aaa\r\nbbb\r\nccc\r\nddd");

    assert_eq!(terminal.scrollback_len(), 2);
    assert_eq!(row_slice_text(terminal.scrollback_row(0).unwrap()), "bbb");
    assert_eq!(row_slice_text(terminal.scrollback_row(1).unwrap()), "ccc");
}

#[test]
fn alternate_screen_scroll_does_not_add_scrollback() {
    let mut terminal = TerminalCore::new(3, 2);
    let _ = terminal.process_pty_input(b"\x1b[?1049hab\r\ncd\r\nef\x1b[?1049l");

    assert_eq!(terminal.scrollback_len(), 0);
}

#[test]
fn line_feed_preserves_column_without_carriage_return() {
    let mut terminal = TerminalCore::new(4, 2);
    let _ = terminal.process_pty_input(b"ab\nc");

    assert_eq!(terminal.grid().cell(0, 0).unwrap().ch, 'a');
    assert_eq!(terminal.grid().cell(1, 0).unwrap().ch, 'b');
    assert_eq!(terminal.grid().cell(2, 1).unwrap().ch, 'c');
}

#[test]
fn next_line_moves_to_column_zero() {
    let mut terminal = TerminalCore::new(4, 2);
    let _ = terminal.process_pty_input(b"ab\x1bEc");

    assert_eq!(terminal.grid().cell(0, 1).unwrap().ch, 'c');
}

#[test]
fn last_column_wrap_is_deferred_until_next_print() {
    let mut terminal = TerminalCore::new(3, 2);
    let _ = terminal.process_pty_input(b"abc");

    assert_eq!(row_text(terminal.grid(), 0), "abc");
    assert_eq!(terminal.grid().cursor().x, 2);
    assert_eq!(terminal.grid().cursor().y, 0);

    let _ = terminal.process_pty_input(b"d");

    assert_eq!(row_text(terminal.grid(), 0), "abc");
    assert_eq!(row_text(terminal.grid(), 1), "d  ");
    assert_eq!(terminal.grid().cursor().x, 1);
    assert_eq!(terminal.grid().cursor().y, 1);
}

#[test]
fn disabled_autowrap_overwrites_last_column() {
    let mut terminal = TerminalCore::new(3, 2);
    let _ = terminal.process_pty_input(b"\x1b[?7labcd");

    assert_eq!(row_text(terminal.grid(), 0), "abd");
    assert_eq!(row_text(terminal.grid(), 1), "   ");
    assert_eq!(terminal.grid().cursor().x, 2);
    assert_eq!(terminal.grid().cursor().y, 0);
}

#[test]
fn wide_characters_leave_spacer_cells() {
    let mut terminal = TerminalCore::new(4, 1);
    let _ = terminal.process_pty_input("表x".as_bytes());

    assert_eq!(terminal.grid().cell(0, 0).unwrap().ch, '表');
    assert!(terminal.grid().cell(0, 0).unwrap().wide);
    assert!(terminal.grid().cell(1, 0).unwrap().spacer);
    assert_eq!(terminal.grid().cell(2, 0).unwrap().ch, 'x');
}

#[test]
fn writing_over_wide_spacer_clears_the_leading_cell() {
    let mut terminal = TerminalCore::new(4, 1);
    let _ = terminal.process_pty_input("表\x1b[1;2Hx".as_bytes());

    assert_eq!(terminal.grid().cell(0, 0).unwrap().ch, ' ');
    assert!(!terminal.grid().cell(0, 0).unwrap().wide);
    assert_eq!(terminal.grid().cell(1, 0).unwrap().ch, 'x');
    assert!(!terminal.grid().cell(1, 0).unwrap().spacer);
}

#[test]
fn sgr_applies_style_to_later_cells() {
    let mut terminal = TerminalCore::new(2, 1);
    let _ = terminal.process_pty_input(b"\x1b[31;1ma");
    let cell = terminal.grid().cell(0, 0).unwrap();

    assert_eq!(cell.style.foreground, crate::Color::Indexed(1));
    assert!(cell.style.bold());
}

#[test]
fn alternate_screen_restores_primary_grid() {
    let mut terminal = TerminalCore::new(4, 1);
    let tick = terminal.process_pty_input(b"abc\x1b[?1049hxyz\x1b[?1049l");

    assert_eq!(terminal.grid().cell(0, 0).unwrap().ch, 'a');
    assert_eq!(terminal.grid().cell(1, 0).unwrap().ch, 'b');
    assert_eq!(terminal.grid().cell(2, 0).unwrap().ch, 'c');
    assert!(
        tick.damage
            .regions
            .iter()
            .any(|region| matches!(region, crate::DamageRegion::Viewport))
    );
}

#[test]
fn scroll_region_limits_line_feed_scrolling() {
    let mut terminal = TerminalCore::new(3, 3);
    let _ = terminal.process_pty_input(b"aa\r\nbb\r\ncc\x1b[2;3r\x1b[3;1H\r\nDD");

    assert_eq!(terminal.grid().cell(0, 0).unwrap().ch, 'a');
    assert_eq!(terminal.grid().cell(0, 1).unwrap().ch, 'c');
    assert_eq!(terminal.grid().cell(0, 2).unwrap().ch, 'D');
}

#[test]
fn extended_sgr_colors_are_applied() {
    let mut terminal = TerminalCore::new(3, 1);
    let _ = terminal.process_pty_input(b"\x1b[38;5;196mA\x1b[48;2;1;2;3mB");

    assert_eq!(
        terminal.grid().cell(0, 0).unwrap().style.foreground,
        crate::Color::Indexed(196)
    );
    assert_eq!(
        terminal.grid().cell(1, 0).unwrap().style.background,
        crate::Color::Rgb(1, 2, 3)
    );
}

#[test]
fn cursor_visibility_mode_updates_grid_cursor() {
    let mut terminal = TerminalCore::new(2, 1);
    let _ = terminal.process_pty_input(b"\x1b[?25l");

    assert!(!terminal.grid().cursor().visible);
}

#[test]
fn cursor_shape_sequence_updates_grid_cursor() {
    let mut terminal = TerminalCore::new(2, 1);

    let _ = terminal.process_pty_input(b"\x1b[6 q");
    assert_eq!(terminal.grid().cursor().shape, crate::CursorShape::Beam);

    let _ = terminal.process_pty_input(b"\x1b[4 q");
    assert_eq!(
        terminal.grid().cursor().shape,
        crate::CursorShape::Underline
    );

    let _ = terminal.process_pty_input(b"\x1b[2 q");
    assert_eq!(terminal.grid().cursor().shape, crate::CursorShape::Block);
}

#[test]
fn synchronized_update_mode_is_tracked() {
    let mut terminal = TerminalCore::new(2, 1);

    let _ = terminal.process_pty_input(b"\x1b[?2026h");
    assert!(terminal.grid().is_synchronized());

    let _ = terminal.process_pty_input(b"\x1b[?2026l");
    assert!(!terminal.grid().is_synchronized());
}

#[test]
fn mouse_modes_are_tracked() {
    let mut terminal = TerminalCore::new(2, 1);

    let _ = terminal.process_pty_input(b"\x1b[?1000;1006h");
    assert_eq!(terminal.mouse().tracking, MouseTracking::Click);
    assert!(terminal.mouse().sgr);

    let _ = terminal.process_pty_input(b"\x1b[?1000l");
    assert_eq!(terminal.mouse().tracking, MouseTracking::None);
    assert!(terminal.mouse().sgr);

    let _ = terminal.process_pty_input(b"\x1b[?1006l");
    assert!(!terminal.mouse().sgr);
}

#[test]
fn bracketed_paste_mode_is_tracked() {
    let mut terminal = TerminalCore::new(2, 1);

    let _ = terminal.process_pty_input(b"\x1b[?2004h");
    assert!(terminal.bracketed_paste());

    let _ = terminal.process_pty_input(b"\x1b[?2004l");
    assert!(!terminal.bracketed_paste());
}

#[test]
fn repeat_sequence_reprints_previous_character() {
    let mut terminal = TerminalCore::new(6, 1);
    let _ = terminal.process_pty_input(b"A\x1b[3b");

    assert_eq!(row_text(terminal.grid(), 0), "AAAA  ");
}

#[test]
fn terminal_queries_emit_pty_responses() {
    let mut terminal = TerminalCore::new(2, 1);

    assert_eq!(
        terminal.process_pty_input(b"\x1b[c").output,
        crate::PRIMARY_DEVICE_ATTRIBUTES
    );
    assert_eq!(
        terminal.process_pty_input(b"\x1b[>c").output,
        crate::SECONDARY_DEVICE_ATTRIBUTES
    );
    assert_eq!(
        terminal.process_pty_input(b"\x1b[>q").output,
        crate::version_reply()
    );
    assert_eq!(
        terminal.process_pty_input(b"\x1b[?u").output,
        crate::KEYBOARD_PROTOCOL_QUERY
    );
    assert_eq!(
        terminal.process_pty_input(b"\x1b[?2026$p").output,
        b"\x1b[?2026;2$y"
    );
    assert_eq!(
        terminal.process_pty_input(b"\x1b]11;?\x07").output,
        crate::DEFAULT_BACKGROUND_REPLY
    );
}

#[test]
fn osc52_clipboard_store_is_reported() {
    let mut terminal = TerminalCore::new(2, 1);
    let tick = terminal.process_pty_input(b"\x1b]52;c;aGVsbG8=\x07");

    assert_eq!(tick.clipboard.len(), 1);
    assert_eq!(tick.clipboard[0].clipboard, b'c');
    assert_eq!(tick.clipboard[0].base64, b"aGVsbG8=");
}

#[test]
fn osc52_empty_selection_defaults_to_clipboard() {
    let mut terminal = TerminalCore::new(2, 1);
    let tick = terminal.process_pty_input(b"\x1b]52;;aGVsbG8=\x07");

    assert_eq!(tick.clipboard[0].clipboard, b'c');
    assert_eq!(tick.clipboard[0].base64, b"aGVsbG8=");
}

#[test]
fn dec_special_graphics_map_tmux_line_drawing() {
    let mut terminal = TerminalCore::new(8, 1);
    let _ = terminal.process_pty_input(b"\x1b(0lqkxmj\x1b(Bq");

    assert_eq!(row_text(terminal.grid(), 0), "┌─┐│└┘q ");
}

#[test]
fn dec_g1_special_graphics_shift_in_and_out() {
    let mut terminal = TerminalCore::new(5, 1);
    let _ = terminal.process_pty_input(b"\x1b)0\x0ex\x0fq");

    assert_eq!(row_text(terminal.grid(), 0), "│q   ");
}

#[test]
fn save_and_restore_cursor_moves_back() {
    let mut terminal = TerminalCore::new(4, 1);
    let _ = terminal.process_pty_input(b"\x1b[3G\x1b[sA\x1b[1G\x1b[uB");

    assert_eq!(terminal.grid().cell(2, 0).unwrap().ch, 'B');
}

fn row_text(grid: &Grid, y: u16) -> String {
    (0..grid.width())
        .map(|x| grid.cell(x, y).unwrap().ch)
        .collect()
}

fn row_slice_text(row: &[crate::Cell]) -> String {
    row.iter().map(|cell| cell.ch).collect()
}
