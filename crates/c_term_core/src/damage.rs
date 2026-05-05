pub type Generation = u64;
const MAX_INCREMENTAL_REGIONS: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DamageRegion {
    Cells {
        x: u16,
        y: u16,
        width: u16,
        height: u16,
    },
    Cursor {
        old: Option<(u16, u16)>,
        new: (u16, u16),
    },
    Scroll {
        top: u16,
        bottom: u16,
        count: u16,
        down: bool,
    },
    Viewport,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DamageBatch {
    pub generation: Generation,
    pub regions: Vec<DamageRegion>,
}

impl DamageBatch {
    pub fn is_empty(&self) -> bool {
        self.regions.is_empty()
    }
}

#[derive(Debug, Default)]
pub struct DamageTracker {
    regions: Vec<DamageRegion>,
    pending_cursor: Option<DamageRegion>,
    marks: usize,
    viewport: bool,
}

impl DamageTracker {
    pub fn mark(&mut self, region: DamageRegion) {
        if self.viewport {
            return;
        }
        if matches!(region, DamageRegion::Viewport) {
            self.collapse_to_viewport();
            return;
        }
        self.marks += 1;
        if self.marks > MAX_INCREMENTAL_REGIONS {
            self.collapse_to_viewport();
            return;
        }

        match region {
            DamageRegion::Cells {
                x,
                y,
                width,
                height,
            } => self.mark_cells(x, y, width, height),
            DamageRegion::Cursor { old, new } => self.mark_cursor(old, new),
            DamageRegion::Scroll {
                top,
                bottom,
                count,
                down,
            } => self.mark_scroll(top, bottom, count, down),
            DamageRegion::Viewport => unreachable!(),
        }
    }

    pub fn drain(&mut self, generation: Generation) -> DamageBatch {
        let regions = if self.viewport {
            self.regions.clear();
            self.pending_cursor = None;
            self.marks = 0;
            self.viewport = false;
            vec![DamageRegion::Viewport]
        } else {
            let mut regions = std::mem::take(&mut self.regions);
            if let Some(cursor) = self.pending_cursor.take() {
                regions.push(cursor);
            }
            self.marks = 0;
            regions
        };
        DamageBatch {
            generation,
            regions,
        }
    }

    pub fn is_empty(&self) -> bool {
        !self.viewport && self.regions.is_empty() && self.pending_cursor.is_none()
    }

    fn mark_cells(&mut self, x: u16, y: u16, width: u16, height: u16) {
        if width == 0 || height == 0 {
            return;
        }

        if let Some(DamageRegion::Cells {
            x: existing_x,
            y: existing_y,
            width: existing_width,
            height: existing_height,
        }) = self.regions.last_mut()
            && *existing_y == y
            && *existing_height == height
        {
            let start = (*existing_x).min(x);
            let end = existing_x
                .saturating_add(*existing_width)
                .max(x.saturating_add(width));
            if x <= existing_x.saturating_add(*existing_width)
                && *existing_x <= x.saturating_add(width)
            {
                *existing_x = start;
                *existing_width = end.saturating_sub(start);
                return;
            }
        }

        self.regions.push(DamageRegion::Cells {
            x,
            y,
            width,
            height,
        });
        self.collapse_if_excessive();
    }

    fn mark_cursor(&mut self, old: Option<(u16, u16)>, new: (u16, u16)) {
        if let Some(DamageRegion::Cursor {
            old: existing_old,
            new: existing_new,
        }) = &mut self.pending_cursor
        {
            if existing_old.is_none() {
                *existing_old = old;
            }
            *existing_new = new;
        } else {
            self.pending_cursor = Some(DamageRegion::Cursor { old, new });
        }
    }

    fn mark_scroll(&mut self, top: u16, bottom: u16, count: u16, down: bool) {
        if let Some(DamageRegion::Scroll {
            top: existing_top,
            bottom: existing_bottom,
            count: existing_count,
            down: existing_down,
        }) = self.regions.last_mut()
            && *existing_top == top
            && *existing_bottom == bottom
            && *existing_down == down
        {
            *existing_count = existing_count.saturating_add(count);
            return;
        }

        self.regions.push(DamageRegion::Scroll {
            top,
            bottom,
            count,
            down,
        });
        self.collapse_if_excessive();
    }

    fn collapse_if_excessive(&mut self) {
        if self.regions.len() > MAX_INCREMENTAL_REGIONS {
            self.collapse_to_viewport();
        }
    }

    fn collapse_to_viewport(&mut self) {
        self.regions.clear();
        self.pending_cursor = None;
        self.marks = 0;
        self.viewport = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_damage_is_preserved() {
        let mut damage = DamageTracker::default();
        damage.mark(DamageRegion::Cells {
            x: 4,
            y: 2,
            width: 3,
            height: 1,
        });
        damage.mark(DamageRegion::Cursor {
            old: Some((4, 2)),
            new: (5, 2),
        });

        assert_eq!(
            damage.drain(1).regions,
            vec![
                DamageRegion::Cells {
                    x: 4,
                    y: 2,
                    width: 3,
                    height: 1,
                },
                DamageRegion::Cursor {
                    old: Some((4, 2)),
                    new: (5, 2),
                },
            ]
        );
    }

    #[test]
    fn viewport_damage_replaces_incremental_damage_on_drain() {
        let mut damage = DamageTracker::default();
        damage.mark(DamageRegion::Cells {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
        });
        damage.mark(DamageRegion::Viewport);

        assert_eq!(damage.drain(1).regions, vec![DamageRegion::Viewport]);
    }

    #[test]
    fn adjacent_scroll_damage_is_coalesced() {
        let mut damage = DamageTracker::default();
        damage.mark(DamageRegion::Scroll {
            top: 0,
            bottom: 9,
            count: 1,
            down: false,
        });
        damage.mark(DamageRegion::Scroll {
            top: 0,
            bottom: 9,
            count: 2,
            down: false,
        });

        assert_eq!(
            damage.drain(1).regions,
            vec![DamageRegion::Scroll {
                top: 0,
                bottom: 9,
                count: 3,
                down: false,
            }]
        );
    }

    #[test]
    fn adjacent_cell_damage_is_coalesced_across_cursor_marks() {
        let mut damage = DamageTracker::default();
        damage.mark(DamageRegion::Cells {
            x: 0,
            y: 1,
            width: 1,
            height: 1,
        });
        damage.mark(DamageRegion::Cursor {
            old: Some((0, 1)),
            new: (1, 1),
        });
        damage.mark(DamageRegion::Cells {
            x: 1,
            y: 1,
            width: 1,
            height: 1,
        });
        damage.mark(DamageRegion::Cursor {
            old: Some((1, 1)),
            new: (2, 1),
        });

        assert_eq!(
            damage.drain(1).regions,
            vec![
                DamageRegion::Cells {
                    x: 0,
                    y: 1,
                    width: 2,
                    height: 1,
                },
                DamageRegion::Cursor {
                    old: Some((0, 1)),
                    new: (2, 1),
                },
            ]
        );
    }

    #[test]
    fn excessive_incremental_damage_collapses_to_viewport() {
        let mut damage = DamageTracker::default();
        for y in 0..=MAX_INCREMENTAL_REGIONS as u16 {
            damage.mark(DamageRegion::Cells {
                x: 0,
                y,
                width: 1,
                height: 1,
            });
        }

        assert_eq!(damage.drain(1).regions, vec![DamageRegion::Viewport]);
    }

    #[test]
    fn excessive_incremental_damage_stops_storing_more_regions() {
        let mut damage = DamageTracker::default();
        for y in 0..=MAX_INCREMENTAL_REGIONS as u16 + 10 {
            damage.mark(DamageRegion::Cells {
                x: 0,
                y,
                width: 1,
                height: 1,
            });
        }

        assert!(damage.regions.is_empty());
        assert_eq!(damage.drain(1).regions, vec![DamageRegion::Viewport]);
    }
}
