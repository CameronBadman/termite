use super::paint::{CellPaint, fill_cell};

#[inline]
pub(super) fn is_private_use_symbol(ch: char) -> bool {
    matches!(ch as u32, 0xe000..=0xf8ff | 0xf0000..=0xffffd | 0x100000..=0x10fffd)
}

#[inline]
pub(super) fn is_special_cell(ch: char) -> bool {
    matches!(
        ch,
        '█' | '▀'
            | '▄'
            | '▌'
            | '▐'
            | '▘'
            | '▝'
            | '▖'
            | '▗'
            | '▚'
            | '▞'
            | '▙'
            | '▛'
            | '▜'
            | '▟'
            | '░'
            | '▒'
            | '▓'
    ) || box_segments(ch).is_some()
}

#[inline]
pub(super) fn draw_special_cell(
    frame: &mut [u8],
    width: usize,
    cell_x: u16,
    cell_y: u16,
    ch: char,
    paint: CellPaint,
) -> bool {
    draw_block_cell_inner(frame, width, cell_x, cell_y, ch, paint, false)
        || draw_shade_cell_inner(frame, width, cell_x, cell_y, ch, paint, false)
        || draw_box_cell_inner(frame, width, cell_x, cell_y, ch, paint, false)
}

#[cfg(test)]
pub(in crate::window_backend) fn draw_block_cell(
    frame: &mut [u8],
    width: usize,
    cell_x: u16,
    cell_y: u16,
    ch: char,
    paint: CellPaint,
) -> bool {
    draw_block_cell_inner(frame, width, cell_x, cell_y, ch, paint, true)
}

fn draw_block_cell_inner(
    frame: &mut [u8],
    width: usize,
    cell_x: u16,
    cell_y: u16,
    ch: char,
    paint: CellPaint,
    fill_background: bool,
) -> bool {
    let metrics = paint.metrics;
    let cell_width = metrics.cell_width as usize;
    let cell_height = metrics.cell_height as usize;
    let mut regions = [(0, 0, 0, 0); 2];
    let region_count = match ch {
        '█' => {
            regions[0] = (0, 0, cell_width, cell_height);
            1
        }
        '▀' => {
            regions[0] = (0, 0, cell_width, cell_height / 2);
            1
        }
        '▄' => {
            regions[0] = (0, cell_height / 2, cell_width, cell_height / 2);
            1
        }
        '▌' => {
            regions[0] = (0, 0, cell_width / 2, cell_height);
            1
        }
        '▐' => {
            regions[0] = (cell_width / 2, 0, cell_width / 2, cell_height);
            1
        }
        '▘' => {
            regions[0] = (0, 0, cell_width / 2, cell_height / 2);
            1
        }
        '▝' => {
            regions[0] = (cell_width / 2, 0, cell_width / 2, cell_height / 2);
            1
        }
        '▖' => {
            regions[0] = (0, cell_height / 2, cell_width / 2, cell_height / 2);
            1
        }
        '▗' => {
            regions[0] = (
                cell_width / 2,
                cell_height / 2,
                cell_width / 2,
                cell_height / 2,
            );
            1
        }
        '▚' => {
            regions[0] = (0, 0, cell_width / 2, cell_height / 2);
            regions[1] = (
                cell_width / 2,
                cell_height / 2,
                cell_width / 2,
                cell_height / 2,
            );
            2
        }
        '▞' => {
            regions[0] = (cell_width / 2, 0, cell_width / 2, cell_height / 2);
            regions[1] = (0, cell_height / 2, cell_width / 2, cell_height / 2);
            2
        }
        '▙' => {
            regions[0] = (0, 0, cell_width / 2, cell_height / 2);
            regions[1] = (0, cell_height / 2, cell_width, cell_height / 2);
            2
        }
        '▛' => {
            regions[0] = (0, 0, cell_width, cell_height / 2);
            regions[1] = (0, cell_height / 2, cell_width / 2, cell_height / 2);
            2
        }
        '▜' => {
            regions[0] = (0, 0, cell_width, cell_height / 2);
            regions[1] = (
                cell_width / 2,
                cell_height / 2,
                cell_width / 2,
                cell_height / 2,
            );
            2
        }
        '▟' => {
            regions[0] = (cell_width / 2, 0, cell_width / 2, cell_height / 2);
            regions[1] = (0, cell_height / 2, cell_width, cell_height / 2);
            2
        }
        _ => return false,
    };

    if fill_background {
        fill_cell(frame, width, cell_x, cell_y, paint.bg, metrics);
    }
    let origin_x = usize::from(cell_x) * metrics.cell_width as usize;
    let origin_y = usize::from(cell_y) * metrics.cell_height as usize;
    for &(x, y, region_width, region_height) in &regions[..region_count] {
        for py in y..y + region_height {
            for px in x..x + region_width {
                let index = ((origin_y + py) * width + origin_x + px) * 4;
                frame[index..index + 4].copy_from_slice(&[
                    paint.fg[0],
                    paint.fg[1],
                    paint.fg[2],
                    0xff,
                ]);
            }
        }
    }
    true
}

#[cfg(test)]
pub(in crate::window_backend) fn draw_shade_cell(
    frame: &mut [u8],
    width: usize,
    cell_x: u16,
    cell_y: u16,
    ch: char,
    paint: CellPaint,
) -> bool {
    draw_shade_cell_inner(frame, width, cell_x, cell_y, ch, paint, true)
}

fn draw_shade_cell_inner(
    frame: &mut [u8],
    width: usize,
    cell_x: u16,
    cell_y: u16,
    ch: char,
    paint: CellPaint,
    fill_background: bool,
) -> bool {
    let metrics = paint.metrics;
    let threshold = match ch {
        '░' => 1,
        '▒' => 2,
        '▓' => 3,
        _ => return false,
    };
    let origin_x = usize::from(cell_x) * metrics.cell_width as usize;
    let origin_y = usize::from(cell_y) * metrics.cell_height as usize;
    for py in 0..metrics.cell_height as usize {
        for px in 0..metrics.cell_width as usize {
            let pattern = (px + py * 3) & 3;
            if pattern >= threshold && !fill_background {
                continue;
            }
            let color = if pattern < threshold {
                paint.fg
            } else {
                paint.bg
            };
            let index = ((origin_y + py) * width + origin_x + px) * 4;
            frame[index..index + 4].copy_from_slice(&[color[0], color[1], color[2], 0xff]);
        }
    }
    true
}

#[cfg(test)]
pub(in crate::window_backend) fn draw_box_cell(
    frame: &mut [u8],
    width: usize,
    cell_x: u16,
    cell_y: u16,
    ch: char,
    paint: CellPaint,
) -> bool {
    draw_box_cell_inner(frame, width, cell_x, cell_y, ch, paint, true)
}

fn draw_box_cell_inner(
    frame: &mut [u8],
    width: usize,
    cell_x: u16,
    cell_y: u16,
    ch: char,
    paint: CellPaint,
    fill_background: bool,
) -> bool {
    let metrics = paint.metrics;
    let Some((left, right, up, down)) = box_segments(ch) else {
        return false;
    };
    let origin_x = usize::from(cell_x) * metrics.cell_width as usize;
    let origin_y = usize::from(cell_y) * metrics.cell_height as usize;
    let center_x = metrics.cell_width as usize / 2;
    let center_y = metrics.cell_height as usize / 2;
    let thickness = 2;

    for py in 0..metrics.cell_height as usize {
        for px in 0..metrics.cell_width as usize {
            let horizontal = py.abs_diff(center_y) < thickness
                && ((left && px <= center_x) || (right && px >= center_x));
            let vertical = px.abs_diff(center_x) < thickness
                && ((up && py <= center_y) || (down && py >= center_y));
            if !(horizontal || vertical || fill_background) {
                continue;
            }
            let color = if horizontal || vertical {
                paint.fg
            } else {
                paint.bg
            };
            let index = ((origin_y + py) * width + origin_x + px) * 4;
            frame[index..index + 4].copy_from_slice(&[color[0], color[1], color[2], 0xff]);
        }
    }
    true
}

pub(in crate::window_backend) fn box_segments(ch: char) -> Option<(bool, bool, bool, bool)> {
    match ch {
        '─' | '━' | '╌' | '╍' | '⎺' | '⎻' | '⎼' | '⎽' => {
            Some((true, true, false, false))
        }
        '╴' => Some((true, false, false, false)),
        '╶' => Some((false, true, false, false)),
        '│' | '┃' | '╎' | '╏' | '┆' | '┇' | '┊' | '┋' => {
            Some((false, false, true, true))
        }
        '╵' => Some((false, false, true, false)),
        '╷' => Some((false, false, false, true)),
        '┌' | '┏' | '╭' => Some((false, true, false, true)),
        '┐' | '┓' | '╮' => Some((true, false, false, true)),
        '└' | '┗' | '╰' => Some((false, true, true, false)),
        '┘' | '┛' | '╯' => Some((true, false, true, false)),
        '├' | '┣' => Some((false, true, true, true)),
        '┤' | '┫' => Some((true, false, true, true)),
        '┬' | '┳' => Some((true, true, false, true)),
        '┴' | '┻' => Some((true, true, true, false)),
        '┼' | '╋' => Some((true, true, true, true)),
        _ => None,
    }
}
