//! Client-owned Vim counts, same-row character motions, and text objects. Read one bounded
//! terminal row per find through the existing API, including offscreen history.
use super::*;
use crossterm::event::KeyModifiers;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

impl ClientShellState {
    pub(super) fn route_copy_vim_motion(
        &mut self,
        key: &crate::input::TerminalKey,
        outcome: &mut ClientShellInput,
    ) -> bool {
        let Some(copy_mode) = self.copy_mode.as_mut() else {
            return false;
        };
        let command = crate::copy_mode::copy_mode_command_char(key.clone());
        let input = &mut copy_mode.vim_input;
        if let Some(mut find) = input.find.take() {
            // Control keys cancel the pending argument; they must not become
            // literal text even when the host supplies generated text.
            if matches!(key.code, KeyCode::Char(_))
                && key.modifiers.difference(KeyModifiers::SHIFT).is_empty()
            {
                let target = key
                    .generated_text
                    .clone()
                    .or_else(|| command.map(|ch| ch.to_string()));
                if let Some(target) = target.filter(|text| {
                    text.graphemes(true).count() == 1 && !text.chars().any(char::is_control)
                }) {
                    find.target = target;
                    // Remember explicit searches even when no occurrence exists.
                    // Reverse repeats must never change the original direction.
                    input.last_find = Some(find.clone());
                    self.copy_operation_queue
                        .push_back(ClientCopyOperation::Row(ClientCopyRowQuery::Find(find)));
                    self.dispatch_next_copy_operation(outcome);
                }
            }
            outcome.repaint = true;
            return true;
        }
        if std::mem::take(&mut input.text_object) {
            // Like Vim, an unknown object key is swallowed with the pending `i`.
            if let Some(object) = command.and_then(ClientCopyTextObject::from_command) {
                self.copy_operation_queue
                    .push_back(ClientCopyOperation::Row(ClientCopyRowQuery::TextObject(
                        object,
                    )));
                self.dispatch_next_copy_operation(outcome);
            }
            outcome.repaint = true;
            return true;
        }
        if let Some(digit) = command.and_then(|ch| ch.to_digit(10)) {
            // Bare 0 remains the existing start-of-row command.
            if digit != 0 || input.count.is_some() {
                input.count = Some(
                    input
                        .count
                        .unwrap_or(0)
                        .saturating_mul(10)
                        .saturating_add(digit),
                );
                outcome.repaint = true;
                return true;
            }
        }
        let pending_count = input.count.take();
        let count = pending_count.unwrap_or(1);
        if matches!(command, Some('f' | 'F')) {
            input.find = Some(ClientCopyFind {
                backward: command == Some('F'),
                count,
                target: String::new(),
            });
            outcome.repaint = true;
            return true;
        }
        if command == Some('i') {
            input.text_object = true;
            outcome.repaint = true;
            return true;
        }
        if matches!(command, Some(';' | ',')) {
            if let Some(mut find) = input.last_find.clone() {
                find.count = count;
                if command == Some(',') {
                    find.backward = !find.backward;
                }
                self.copy_operation_queue
                    .push_back(ClientCopyOperation::Row(ClientCopyRowQuery::Find(find)));
                self.dispatch_next_copy_operation(outcome);
            }
            outcome.repaint = true;
            return true;
        }
        let row_delta = match command {
            Some('j') => Some(i64::from(count)),
            Some('k') => Some(-i64::from(count)),
            _ if key.modifiers.is_empty() => match key.code {
                KeyCode::Down => Some(i64::from(count)),
                KeyCode::Up => Some(-i64::from(count)),
                _ => None,
            },
            _ => None,
        };
        if let Some(delta) = row_delta {
            self.move_copy_cursor(delta, 0, outcome);
            return true;
        }
        // Escape cancels an unfinished count without clearing the selection.
        if pending_count.is_some() && key.code == KeyCode::Esc {
            outcome.repaint = true;
            return true;
        }
        false
    }

    pub(super) fn insert_copy_find_text(
        &mut self,
        text: &str,
        outcome: &mut ClientShellInput,
    ) -> bool {
        if self.mode != ClientShellMode::Copy
            || self.overlay.is_some()
            || self.popup_terminal_id.is_some()
            || self.popup_pending
            || !(self
                .copy_mode
                .as_ref()
                .is_some_and(|mode| mode.vim_input.find.is_some())
                || (self.copy_operation_in_flight
                    && self.copy_input_queue.back().is_some_and(|key| {
                        matches!(
                            crate::copy_mode::copy_mode_command_char(key.clone()),
                            Some('f' | 'F')
                        )
                    })))
        {
            return false;
        }
        if let Some(target) = text
            .graphemes(true)
            .next()
            .filter(|target| !target.chars().any(char::is_control))
        {
            let key = crate::input::TerminalKey::new(KeyCode::Char(' '), KeyModifiers::NONE)
                .with_generated_text(Some(target.to_owned()));
            self.handle_key(key, outcome);
        }
        true
    }

    pub(super) fn complete_copy_find(
        &mut self,
        request: ClientCopyFindRequest,
        result: Result<crate::api::schema::ResponseResult, ClientShellEndpointError>,
    ) -> (bool, Vec<ClientShellAction>) {
        let ClientCopyFindRequest {
            pane_id,
            origin,
            content_revision,
            session_generation,
            geometry,
            alternate_screen_active,
            query,
        } = request;
        if self.copy_session_generation != session_generation {
            return (false, Vec::new());
        }
        let mut outcome = ClientShellInput::default();
        let valid = self.copy_mode.as_ref().is_some_and(|mode| {
            mode.pane_id == pane_id
                && mode.cursor == origin
                && mode.content_revision == content_revision
                && mode.geometry == geometry
                && mode.alternate_screen_active == alternate_screen_active
        });
        let accepted = match result {
            Ok(crate::api::schema::ResponseResult::PaneSelection {
                pane_id: returned,
                text,
            }) if returned == pane_id && valid => {
                let col = match &query {
                    ClientCopyRowQuery::Find(find) => find_column(&text, origin.col, find),
                    ClientCopyRowQuery::TextObject(object) => {
                        super::copy_text_objects::text_object_columns(&text, origin.col, *object)
                            .map(|(start, end)| {
                                // The object replaces any selection, as `viw` does in Vim.
                                if let Some(copy_mode) = self.copy_mode.as_mut() {
                                    copy_mode.selection = Some(ClientCopySelection::Character {
                                        anchor: crate::api::schema::PaneTextPoint {
                                            row: origin.row,
                                            col: start,
                                        },
                                    });
                                }
                                end
                            })
                    }
                }
                .unwrap_or(origin.col);
                self.apply_copy_motion_target(
                    &pane_id,
                    origin,
                    crate::api::schema::PaneTextPoint {
                        row: origin.row,
                        col,
                    },
                    content_revision,
                    &mut outcome,
                )
            }
            _ => false,
        };
        self.complete_copy_operation(session_generation, accepted, &mut outcome);
        (outcome.repaint, outcome.actions)
    }
}

fn find_column(text: &str, origin: u16, find: &ClientCopyFind) -> Option<u16> {
    let mut col = 0u16;
    let columns = text
        .graphemes(true)
        .filter_map(|grapheme| {
            let start = col;
            col = col
                .saturating_add(UnicodeWidthStr::width(grapheme).min(usize::from(u16::MAX)) as u16);
            // A base character also finds that character with composing marks.
            (grapheme.starts_with(&find.target)
                && if find.backward {
                    start < origin
                } else {
                    start > origin
                })
            .then_some(start)
        })
        .collect::<Vec<_>>();
    let index = usize::try_from(find.count.saturating_sub(1)).ok()?;
    if find.backward {
        columns.into_iter().rev().nth(index)
    } else {
        columns.into_iter().nth(index)
    }
}
