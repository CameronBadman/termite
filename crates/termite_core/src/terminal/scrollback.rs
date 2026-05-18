use crate::ParserAdapter;

use super::TerminalCore;

impl<P> TerminalCore<P>
where
    P: ParserAdapter,
{
    pub(super) fn append_scrollback_rows(
        &mut self,
        rows: Vec<Vec<crate::Cell>>,
    ) -> Vec<Vec<crate::Cell>> {
        let mut recycled_rows = Vec::new();
        if rows.is_empty() || self.scrollback_capacity == 0 {
            recycled_rows.extend(rows);
            return recycled_rows;
        }

        if rows.len() >= self.scrollback_capacity {
            let keep_from = rows.len() - self.scrollback_capacity;
            recycled_rows.extend(self.scrollback.drain(..));
            for (index, row) in rows.into_iter().enumerate() {
                if index < keep_from {
                    recycled_rows.push(row);
                } else {
                    self.scrollback.push_back(row);
                }
            }
            return recycled_rows;
        }

        let overflow = self
            .scrollback
            .len()
            .saturating_add(rows.len())
            .saturating_sub(self.scrollback_capacity);
        if overflow > 0 {
            recycled_rows.extend(self.scrollback.drain(..overflow));
        }
        self.scrollback.extend(rows);
        recycled_rows
    }
}
