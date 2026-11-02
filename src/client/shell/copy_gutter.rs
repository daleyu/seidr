//! Client-only copy-mode projection. Terminal geometry and selection coordinates
//! stay unchanged; only the focused pane's displayed cells are shifted/cropped.
use super::*;

#[derive(Clone, Copy, Debug)]
pub(super) struct ClientCopyGutter {
    pub(super) rect: Rect,
    pub(super) width: u16,
    pub(super) left_col: u16,
    top_row: u32,
}

impl ClientCopyGutter {
    pub(super) fn new(mode: &ClientCopyModeState, hit: &PaneHit) -> Option<Self> {
        let rect = hit.inner_rect;
        if rect.is_empty() {
            return None;
        }
        let top_row = mode
            .max_offset_from_bottom
            .saturating_sub(mode.offset_from_bottom)
            .min(u32::MAX as usize) as u32;
        let bottom_row = top_row.saturating_add(u32::from(rect.height - 1));
        let max_distance = mode
            .cursor
            .row
            .abs_diff(top_row)
            .max(mode.cursor.row.abs_diff(bottom_row));
        let digits = max_distance.checked_ilog10().unwrap_or(0) as u16 + 1;
        let width = digits + 1;
        // Keep at least one text cell in very narrow splits.
        if rect.width <= width {
            return None;
        }
        let text_width = rect.width - width;
        // Leave room for a wide cursor glyph where the terminal has another cell.
        let left_col = mode
            .cursor
            .col
            .saturating_sub(text_width.saturating_sub(2))
            .min(width);
        Some(Self {
            rect,
            width,
            left_col,
            top_row,
        })
    }

    pub(super) fn source_col(self, screen_column: u16) -> u16 {
        screen_column
            .saturating_sub(self.rect.x.saturating_add(self.width))
            .min(self.rect.width - self.width - 1)
            .saturating_add(self.left_col)
    }

    pub(super) fn contains_number(self, column: u16, row: u16) -> bool {
        contains(
            Rect::new(self.rect.x, self.rect.y, self.width, self.rect.height),
            (column, row),
        )
    }

    pub(super) fn render(
        self,
        frame: &mut FrameData,
        mode: &ClientCopyModeState,
        palette: &Palette,
    ) {
        let blank = crate::protocol::CellData::from_ratatui_cell(
            ratatui::buffer::Cell::default()
                .set_style(Style::default().fg(palette.overlay0).bg(palette.panel_bg)),
        );
        let stride = usize::from(frame.width);
        let text_width = usize::from(self.rect.width - self.width);
        for y in self.rect.y..self.rect.bottom().min(frame.height) {
            let start = usize::from(y) * stride + usize::from(self.rect.x);
            let end = start + usize::from(self.rect.width);
            let Some(row) = frame.cells.get_mut(start..end) else {
                continue;
            };
            let source = usize::from(self.left_col);
            let target = usize::from(self.width);
            let starts_in_wide_character =
                source > 0 && UnicodeWidthStr::width(row[source - 1].symbol.as_str()) > 1;
            // Move the original cells, including hyperlinks and highlight styles,
            // without allocating a second pane-sized buffer.
            if source <= target {
                for col in (0..text_width).rev() {
                    let (before, after) = row.split_at_mut(target + col);
                    if source + col < before.len() {
                        after[0].clone_from(&before[source + col]);
                    }
                }
            } else {
                for col in 0..text_width {
                    let (before, after) = row.split_at_mut(source + col);
                    before[target + col].clone_from(&after[0]);
                }
            }
            // Do not display half a wide character at either clipped edge.
            if starts_in_wide_character {
                row[target].clone_from(&blank);
            }
            let last = row.len() - 1;
            if UnicodeWidthStr::width(row[last].symbol.as_str()) > 1 {
                row[last].clone_from(&blank);
            }
            for cell in &mut row[..target] {
                cell.clone_from(&blank);
            }
            let absolute_row = self.top_row.saturating_add(u32::from(y - self.rect.y));
            let mut distance = absolute_row.abs_diff(mode.cursor.row);
            let mut digit_col = target - 2;
            loop {
                let cell = &mut row[digit_col];
                cell.symbol.clear();
                cell.symbol.push(char::from(b'0' + (distance % 10) as u8));
                if absolute_row == mode.cursor.row {
                    cell.fg = crate::protocol::color_to_u32(palette.accent);
                }
                distance /= 10;
                if distance == 0 || digit_col == 0 {
                    break;
                }
                digit_col -= 1;
            }
        }
    }
}

impl ClientShellState {
    pub(super) fn copy_gutter_for_hit(&self, hit: &PaneHit) -> Option<ClientCopyGutter> {
        self.copy_gutter.filter(|gutter| {
            gutter.rect == hit.inner_rect
                && self
                    .copy_mode
                    .as_ref()
                    .is_some_and(|mode| mode.pane_id == hit.pane_id)
        })
    }

    pub(super) fn pane_source_column(&self, hit: &PaneHit, column: u16) -> u16 {
        self.copy_gutter_for_hit(hit).map_or_else(
            || column.saturating_sub(hit.inner_rect.x),
            |gutter| gutter.source_col(column),
        )
    }
}
