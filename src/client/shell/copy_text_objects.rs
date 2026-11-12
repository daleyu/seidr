//! Vim inner text objects for copy mode (`iw`, `i(`, `i"`, ...). They resolve
//! against one terminal row, read through the same bounded row request as
//! character finds, and replace the selection with the object's cells.
use super::*;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

impl ClientCopyTextObject {
    pub(super) fn from_command(command: char) -> Option<Self> {
        Some(match command {
            'w' => Self::Word,
            'W' => Self::BigWord,
            'b' | '(' | ')' => Self::Pair('(', ')'),
            'B' | '{' | '}' => Self::Pair('{', '}'),
            '[' | ']' => Self::Pair('[', ']'),
            '<' | '>' => Self::Pair('<', '>'),
            '`' | '\'' | '"' => Self::Quote(command),
            'l' => Self::Line,
            _ => return None,
        })
    }
}

struct Cell<'a> {
    col: u16,
    width: u16,
    text: &'a str,
}

impl Cell<'_> {
    fn is(&self, ch: char) -> bool {
        let mut chars = self.text.chars();
        chars.next() == Some(ch) && chars.next().is_none()
    }

    fn is_blank(&self) -> bool {
        self.text.chars().all(char::is_whitespace)
    }

    fn word_class(&self, big: bool) -> u8 {
        if self.is_blank() {
            0
        } else if big
            || self
                .text
                .chars()
                .next()
                .is_some_and(|ch| ch.is_alphanumeric() || ch == '_')
        {
            2
        } else {
            1
        }
    }
}

/// Returns the inclusive start and end columns of the object around `origin`.
pub(super) fn text_object_columns(
    text: &str,
    origin: u16,
    object: ClientCopyTextObject,
) -> Option<(u16, u16)> {
    let mut col = 0u16;
    let cells = text
        .graphemes(true)
        .map(|grapheme| {
            let width = UnicodeWidthStr::width(grapheme).min(usize::from(u16::MAX)) as u16;
            let cell = Cell {
                col,
                width,
                text: grapheme,
            };
            col = col.saturating_add(width);
            cell
        })
        .collect::<Vec<_>>();
    // A cursor right of the row's text sits past the last cell.
    let index = cells
        .iter()
        .position(|cell| origin < cell.col.saturating_add(cell.width.max(1)))
        .unwrap_or(cells.len());
    let (first, last) = match object {
        ClientCopyTextObject::Word => word(&cells, index, false)?,
        ClientCopyTextObject::BigWord => word(&cells, index, true)?,
        ClientCopyTextObject::Pair(open, close) => pair(&cells, index, open, close)?,
        ClientCopyTextObject::Quote(quote) => quoted(&cells, index, quote)?,
        ClientCopyTextObject::Line => (
            cells.iter().position(|cell| !cell.is_blank())?,
            cells.iter().rposition(|cell| !cell.is_blank())?,
        ),
    };
    Some((cells.get(first)?.col, cells.get(last)?.col))
}

fn word(cells: &[Cell<'_>], index: usize, big: bool) -> Option<(usize, usize)> {
    let class = cells.get(index)?.word_class(big);
    let same = |cell: &Cell<'_>| cell.word_class(big) == class;
    let first = cells[..index]
        .iter()
        .rposition(|cell| !same(cell))
        .map_or(0, |other| other + 1);
    let last = cells[index..]
        .iter()
        .position(|cell| !same(cell))
        .map_or(cells.len() - 1, |other| index + other - 1);
    Some((first, last))
}

fn pair(cells: &[Cell<'_>], index: usize, open: char, close: char) -> Option<(usize, usize)> {
    let matching_close = |open_index: usize| {
        let mut depth = 0usize;
        for (offset, cell) in cells[open_index + 1..].iter().enumerate() {
            if cell.is(open) {
                depth += 1;
            } else if cell.is(close) {
                if depth == 0 {
                    return Some(open_index + 1 + offset);
                }
                depth -= 1;
            }
        }
        None
    };
    // The innermost pair around the cursor; a cursor on either delimiter
    // belongs to that delimiter's pair.
    let enclosing = || {
        let on_close = cells.get(index).is_some_and(|cell| cell.is(close));
        let scan_end = if on_close {
            index
        } else {
            (index + 1).min(cells.len())
        };
        let mut depth = 0usize;
        for candidate in (0..scan_end).rev() {
            let cell = &cells[candidate];
            if cell.is(close) {
                depth += 1;
            } else if cell.is(open) {
                if depth == 0 {
                    let close_index = matching_close(candidate)?;
                    return (close_index >= index).then_some((candidate, close_index));
                }
                depth -= 1;
            }
        }
        None
    };
    // Otherwise fall forward to the next pair on the row.
    let following = || {
        cells
            .iter()
            .enumerate()
            .skip(index + 1)
            .filter(|(_, cell)| cell.is(open))
            .find_map(|(candidate, _)| Some((candidate, matching_close(candidate)?)))
    };
    let (open_index, close_index) = enclosing().or_else(following)?;
    inner(open_index, close_index)
}

fn quoted(cells: &[Cell<'_>], index: usize, quote: char) -> Option<(usize, usize)> {
    // Backslash-escaped quotes do not delimit, matching Vim's 'quoteescape'.
    let quotes = cells
        .iter()
        .enumerate()
        .filter(|(position, cell)| {
            cell.is(quote) && !(*position > 0 && cells[*position - 1].is('\\'))
        })
        .map(|(position, _)| position)
        .collect::<Vec<_>>();
    let (open_index, close_index) = match quotes.iter().position(|position| *position == index) {
        // On a quote: quotes pair up from the start of the row.
        Some(ordinal) if ordinal % 2 == 0 => (index, *quotes.get(ordinal + 1)?),
        Some(ordinal) => (quotes[ordinal - 1], index),
        None => {
            let next = quotes.iter().position(|position| *position > index)?;
            match next.checked_sub(1) {
                Some(previous) => (quotes[previous], quotes[next]),
                // No quote before the cursor: use the first quoted run ahead.
                None => (quotes[next], *quotes.get(next + 1)?),
            }
        }
    };
    inner(open_index, close_index)
}

fn inner(open_index: usize, close_index: usize) -> Option<(usize, usize)> {
    (close_index > open_index + 1).then(|| (open_index + 1, close_index - 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn object(keys: char, text: &str, origin: u16) -> Option<(u16, u16)> {
        text_object_columns(
            text,
            origin,
            ClientCopyTextObject::from_command(keys).expect("text object"),
        )
    }

    #[test]
    fn words_follow_vim_classes() {
        let text = "foo.bar_1  baz";
        assert_eq!(object('w', text, 1), Some((0, 2)));
        assert_eq!(object('w', text, 3), Some((3, 3)));
        assert_eq!(object('w', text, 5), Some((4, 8)));
        assert_eq!(object('w', text, 9), Some((9, 10)));
        assert_eq!(object('w', text, 13), Some((11, 13)));
        assert_eq!(object('W', text, 5), Some((0, 8)));
        assert_eq!(object('W', text, 10), Some((9, 10)));
        assert_eq!(object('w', text, 40), None);
    }

    #[test]
    fn words_use_display_columns_for_wide_text() {
        // Each CJK grapheme covers two cells.
        assert_eq!(object('w', "a 日本 b", 3), Some((2, 4)));
        assert_eq!(object('w', "a 日本 b", 4), Some((2, 4)));
    }

    #[test]
    fn pairs_select_the_innermost_enclosing_contents() {
        let text = "f(a, (b), c) x";
        assert_eq!(object('(', text, 3), Some((2, 10)));
        assert_eq!(object(')', text, 6), Some((6, 6)));
        assert_eq!(object('b', text, 9), Some((2, 10)));
        // On a delimiter the cursor belongs to that delimiter's pair.
        assert_eq!(object('(', text, 1), Some((2, 10)));
        assert_eq!(object('(', text, 5), Some((6, 6)));
        assert_eq!(object('(', text, 7), Some((6, 6)));
        assert_eq!(object('(', text, 11), Some((2, 10)));
        // Before any pair, fall forward; after the last one there is nothing.
        assert_eq!(object('(', text, 0), Some((2, 10)));
        assert_eq!(object('(', text, 13), None);
        assert_eq!(object('(', "() (", 0), None);
        assert_eq!(object('(', "a) (b", 4), None);
    }

    #[test]
    fn every_pair_alias_resolves() {
        for (keys, text) in [("B{}", "{ab}"), ("[]", "[ab]"), ("<>", "<ab>")] {
            for key in keys.chars() {
                assert_eq!(object(key, text, 2), Some((1, 2)), "{key}");
            }
        }
    }

    #[test]
    fn quotes_pair_from_the_row_start_and_skip_escapes() {
        let text = r#"a "bc" d "e\"f""#;
        assert_eq!(object('"', text, 3), Some((3, 4)));
        assert_eq!(object('"', text, 2), Some((3, 4)));
        assert_eq!(object('"', text, 5), Some((3, 4)));
        assert_eq!(object('"', text, 0), Some((3, 4)));
        assert_eq!(object('"', text, 12), Some((10, 13)));
        // Between two strings Vim takes the text between the nearest quotes.
        assert_eq!(object('"', text, 7), Some((6, 8)));
        assert_eq!(object('"', text, 15), None);
        // Off a quote, the nearest quotes on either side win, apostrophes included.
        assert_eq!(object('\'', "it's 'x'", 6), Some((6, 6)));
        assert_eq!(object('`', "run `ls -a`", 0), Some((5, 9)));
        assert_eq!(object('"', r#""""#, 0), None);
    }

    #[test]
    fn line_trims_surrounding_blanks() {
        assert_eq!(object('l', "  ab cd  ", 0), Some((2, 6)));
        assert_eq!(object('l', "  ab cd  ", 30), Some((2, 6)));
        assert_eq!(object('l', "   ", 1), None);
    }
}
