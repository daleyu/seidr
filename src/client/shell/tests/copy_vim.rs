use super::*;
use crate::api::schema::{Method, PaneTextPoint, ResponseResult};
use crate::input::{TerminalKey, TextCommit};

fn visual() -> ClientShellState {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    let mut projected = surface();
    projected.panes[0].scroll = Some(crate::protocol::PaneSurfaceScrollMetrics {
        offset_from_bottom: 20,
        max_offset_from_bottom: 40,
        viewport_rows: 2,
    });
    state.set_pane_surface(projected);
    state.compose(106, 20).expect("frame");
    state.handle_input_bytes(b"\x02[v");
    assert_eq!(state.mode, ClientShellMode::Copy);
    state
}

fn cursor(state: &ClientShellState) -> PaneTextPoint {
    state.copy_mode.as_ref().expect("copy mode").cursor
}

fn request_id(input: &ClientShellInput) -> String {
    input
        .actions
        .iter()
        .find_map(|action| match action {
            ClientShellAction::Endpoint { request, .. }
                if matches!(request.method, Method::PaneSelectionRead(_)) =>
            {
                Some(request.id.clone())
            }
            _ => None,
        })
        .expect("row read")
}

fn reply(state: &mut ClientShellState, id: &str, text: &str) -> Vec<ClientShellAction> {
    state
        .handle_endpoint_result(
            "boot-1",
            id,
            Ok(ResponseResult::PaneSelection {
                pane_id: "pane_1".into(),
                text: text.into(),
            }),
        )
        .1
}

#[test]
fn copy_vim_counts_move_display_rows_and_extend_both_selection_types() {
    for linewise in [false, true] {
        let mut state = visual();
        if linewise {
            state.handle_input_bytes(b"V");
        }
        let origin = cursor(&state);
        state.handle_input_bytes(b"10j");
        assert_eq!(
            cursor(&state),
            PaneTextPoint {
                row: origin.row + 10,
                ..origin
            }
        );
        state.handle_input_bytes(b"10k");
        assert_eq!(cursor(&state), origin);
        state.handle_input_bytes(b"2j3k");
        assert_eq!(cursor(&state).row, origin.row - 1);
        let range = state.selection.as_ref().expect("selection").ordered_cells();
        assert_eq!(range.0 .0, origin.row - 1);
        assert_eq!(range.1 .0, origin.row);
        // A count applies once, and saturates safely at history boundaries.
        state.handle_input_bytes(b"j999999999999999999999999999999999999999999j");
        assert_eq!(cursor(&state).row, 41);
        state.handle_input_bytes(b"999999999999999999999999999999999999999999k");
        assert_eq!(cursor(&state).row, 0);
        state.handle_input_bytes(b"l0");
        assert_eq!(cursor(&state).col, 0);
    }
}

#[test]
fn copy_vim_find_is_counted_directional_inclusive_and_row_bounded() {
    for (keys, origin_col, expected) in [
        ("fa", 0, 1),
        ("2fa", 0, 3),
        ("3fa", 0, 0),
        ("Fa", 3, 1),
        ("2Fa", 3, 3),
        ("fx", 0, 2),
        ("fz", 1, 1),
    ] {
        let mut state = visual();
        state.copy_mode.as_mut().unwrap().cursor.col = origin_col;
        state.handle_input_bytes(b"v");
        let origin = cursor(&state);
        let input = state.handle_input_bytes(keys.as_bytes());
        let id = request_id(&input);
        assert!(input.actions.iter().any(|action| matches!(action,
            ClientShellAction::Endpoint { request, .. }
                if matches!(&request.method, Method::PaneSelectionRead(params)
                    if params.anchor == PaneTextPoint { row: origin.row, col: 0 }
                        && params.cursor == PaneTextPoint { row: origin.row, col: 3 }
                        && params.content_revision == Some(0))
        )));
        reply(&mut state, &id, "xaxa");
        assert_eq!(
            cursor(&state),
            PaneTextPoint {
                col: expected,
                ..origin
            },
            "{keys}"
        );
        assert_eq!(
            state.selection.as_ref().unwrap().ordered_cells(),
            (
                (origin.row, origin_col.min(expected)),
                (origin.row, origin_col.max(expected))
            )
        );
    }
}

#[test]
fn copy_vim_find_accepts_shifted_keys_generated_text_and_composing_commits() {
    let mut state = visual();
    state.copy_mode.as_mut().unwrap().cursor.col = 3;
    state.handle_raw_events(vec![RawInputEvent::Key(TerminalKey::new(
        KeyCode::Char('f'),
        KeyModifiers::SHIFT,
    ))]);
    let input = state.handle_raw_events(vec![RawInputEvent::Key(
        TerminalKey::new(KeyCode::Char('x'), KeyModifiers::NONE)
            .with_generated_text(Some("界".into())),
    )]);
    reply(&mut state, &request_id(&input), "a界e\u{301}");
    assert_eq!(cursor(&state).col, 1);

    state.handle_input_bytes(b"f");
    let input = state.handle_raw_events(vec![RawInputEvent::Text(TextCommit::new("e\u{301}"))]);
    reply(&mut state, &request_id(&input), "a界e\u{301}");
    assert_eq!(cursor(&state).col, 3);
}

#[test]
fn copy_vim_counts_support_arrows_and_reset_before_search_prompts() {
    let mut state = visual();
    let origin = cursor(&state);
    state.handle_input_bytes(b"10");
    state.handle_raw_events(vec![RawInputEvent::Key(TerminalKey::new(
        KeyCode::Up,
        KeyModifiers::NONE,
    ))]);
    assert_eq!(cursor(&state).row, origin.row - 10);
    state.handle_input_bytes(b"10");
    state.handle_raw_events(vec![RawInputEvent::Key(TerminalKey::new(
        KeyCode::Down,
        KeyModifiers::NONE,
    ))]);
    assert_eq!(cursor(&state), origin);
    state.handle_input_bytes(b"10/123");
    let mode = state.copy_mode.as_ref().unwrap();
    assert!(mode.vim_input.count.is_none());
    assert_eq!(mode.search_prompt.as_ref().unwrap().query.as_str(), "123");
}

#[test]
fn copy_vim_committed_find_argument_waits_behind_an_endpoint_motion() {
    let mut state = visual();
    let origin = cursor(&state);
    let motion = state.handle_input_bytes(b"w");
    let motion_id = match &motion.actions[0] {
        ClientShellAction::Endpoint { request, .. } => request.id.clone(),
        _ => panic!("word motion"),
    };
    state.handle_input_bytes(b"f");
    state.handle_raw_events(vec![RawInputEvent::Text(TextCommit::new("e\u{301}"))]);
    let (_, actions) = state.handle_endpoint_result(
        "boot-1",
        &motion_id,
        Ok(ResponseResult::PaneCopyMotion {
            pane_id: "pane_1".into(),
            cursor: origin,
            content_revision: 0,
        }),
    );
    let input = ClientShellInput {
        actions,
        ..ClientShellInput::default()
    };
    reply(&mut state, &request_id(&input), "a界e\u{301}");
    assert_eq!(cursor(&state).col, 3);
}

#[test]
fn copy_vim_escape_cancels_pending_commands_and_digits_can_be_find_targets() {
    for keys in [b"10".as_slice(), b"2f".as_slice()] {
        let mut state = visual();
        let origin = cursor(&state);
        state.handle_input_bytes(keys);
        state.handle_raw_events(vec![RawInputEvent::Key(
            TerminalKey::new(KeyCode::Esc, KeyModifiers::NONE)
                .with_generated_text(Some("x".into())),
        )]);
        state.handle_input_bytes(b"j");
        assert_eq!(cursor(&state).row, origin.row + 1);
        assert!(state.copy_mode.as_ref().unwrap().selection.is_some());
    }
    let mut state = visual();
    let input = state.handle_input_bytes(b"f0");
    reply(&mut state, &request_id(&input), "ab00");
    assert_eq!(cursor(&state).col, 2);
    let input = state.handle_input_bytes(b"f ");
    reply(&mut state, &request_id(&input), "a0a ");
    assert_eq!(cursor(&state).col, 3);
}

#[test]
fn copy_vim_find_serializes_repeats_counts_and_yank_after_the_reply() {
    let mut state = visual();
    state.copy_mode.as_mut().unwrap().cursor.col = 0;
    state.handle_input_bytes(b"v");
    let origin = cursor(&state);
    let first = state.handle_input_bytes(b"fafa10jy");
    let actions = reply(&mut state, &request_id(&first), "xaxa");
    assert_eq!(cursor(&state).col, 1);
    let second = ClientShellInput {
        actions,
        ..ClientShellInput::default()
    };
    let actions = reply(&mut state, &request_id(&second), "xaxa");
    assert!(state.copy_mode.is_none());
    assert!(actions.iter().any(|action| matches!(action,
        ClientShellAction::Endpoint { request, .. }
            if matches!(&request.method, Method::PaneSelectionRead(params)
                if params.anchor == PaneTextPoint { col: 0, ..origin }
                    && params.cursor == PaneTextPoint { row: origin.row + 10, col: 3 })
    )));
}

#[test]
fn copy_vim_find_repeats_preserve_original_direction_and_override_counts() {
    for (initial, start, steps) in [
        ("fa", 0, vec![("2;", 3), (",", 2), (";", 3), ("2,", 1)]),
        ("Fa", 3, vec![(";", 1), (",", 2), (";", 1), ("2,", 3)]),
        ("2fa", 0, vec![(";", 3), (",", 2), (";", 3)]),
    ] {
        let mut state = visual();
        state.copy_mode.as_mut().unwrap().cursor.col = start;
        state.handle_input_bytes(b"v");
        let origin = cursor(&state);
        let input = state.handle_input_bytes(initial.as_bytes());
        reply(&mut state, &request_id(&input), "aaaa");
        for (keys, expected) in steps {
            let input = state.handle_input_bytes(keys.as_bytes());
            reply(&mut state, &request_id(&input), "aaaa");
            assert_eq!(cursor(&state).col, expected, "{initial} then {keys}");
            assert_eq!(
                state.selection.as_ref().unwrap().ordered_cells(),
                (
                    (origin.row, start.min(expected)),
                    (origin.row, start.max(expected)),
                )
            );
        }
    }
}

#[test]
fn copy_vim_repeat_handles_missing_history_failed_finds_and_cancellation() {
    let mut state = visual();
    let origin = cursor(&state);
    let input = state.handle_input_bytes(b";2,");
    assert!(input.actions.is_empty());
    assert_eq!(cursor(&state), origin);

    let input = state.handle_input_bytes(b"fz");
    reply(&mut state, &request_id(&input), "aaaa");
    assert_eq!(cursor(&state), origin);
    state.handle_input_bytes(b"f");
    state.handle_raw_events(vec![RawInputEvent::Key(TerminalKey::new(
        KeyCode::Esc,
        KeyModifiers::NONE,
    ))]);
    let input = state.handle_input_bytes(b";");
    reply(&mut state, &request_id(&input), "aazz");
    assert_eq!(cursor(&state).col, 2);

    // Repeat keys remain literal arguments while a character find is pending.
    for (keys, row) in [("f;", "aa;z"), ("f,", "aa,z")] {
        state.copy_mode.as_mut().unwrap().cursor.col = 0;
        let input = state.handle_input_bytes(keys.as_bytes());
        reply(&mut state, &request_id(&input), row);
        assert_eq!(cursor(&state).col, 2);
    }
}

#[test]
fn copy_vim_repeat_serializes_rapid_input_and_remembers_unicode() {
    let mut state = visual();
    state.copy_mode.as_mut().unwrap().cursor.col = 0;
    let mut input = state.handle_input_bytes("fé;,;".as_bytes());
    for expected in [1, 2, 1, 2] {
        let actions = reply(&mut state, &request_id(&input), "aééé");
        assert_eq!(cursor(&state).col, expected);
        input = ClientShellInput {
            actions,
            ..ClientShellInput::default()
        };
    }
    assert!(!state.copy_operation_in_flight);
    assert!(state.copy_input_queue.is_empty());
}

#[test]
fn copy_vim_find_ignores_stale_content_coordinates_sessions_and_errors() {
    for invalidation in 0..5 {
        let mut state = visual();
        let origin = cursor(&state);
        let input = state.handle_input_bytes(b"fa");
        let id = request_id(&input);
        match invalidation {
            0 => state.copy_mode.as_mut().unwrap().content_revision += 2,
            1 => state.copy_mode.as_mut().unwrap().geometry.0 += 1,
            2 => state.copy_mode.as_mut().unwrap().alternate_screen_active = true,
            3 => state.copy_session_generation += 1,
            _ => {
                state.handle_endpoint_result(
                    "boot-1",
                    &id,
                    Err(ClientShellEndpointError {
                        code: Some("stale_content".into()),
                        message: "changed".into(),
                    }),
                );
            }
        }
        reply(&mut state, &id, "xaxa");
        assert_eq!(cursor(&state), origin);
    }
}

#[test]
fn copy_vim_text_objects_replace_the_selection_on_the_cursor_row() {
    for (keys, text, origin_col, expected) in [
        ("iw", "ab cd", 3, Some((3, 4))),
        ("viw", "ab cd", 0, Some((0, 1))),
        ("Viw", "ab cd", 4, Some((3, 4))),
        ("iW", "a.b c", 1, Some((0, 2))),
        ("i(", "f(ab)", 3, Some((2, 3))),
        ("ib", "f(ab)", 0, Some((2, 3))),
        ("i}", "{ab} ", 1, Some((1, 2))),
        ("iB", "{ab} ", 2, Some((1, 2))),
        ("i]", "x[ab]", 2, Some((2, 3))),
        ("i>", "<ab> ", 2, Some((1, 2))),
        ("i\"", "\"ab\" ", 2, Some((1, 2))),
        ("i'", "'ab' ", 1, Some((1, 2))),
        ("i`", "`ab` ", 3, Some((1, 2))),
        ("il", " abc ", 0, Some((1, 3))),
        // No object under the cursor leaves the cursor and selection alone.
        ("i(", "abcde", 2, None),
        ("vi\"", "abcde", 2, None),
    ] {
        let mut state = visual();
        state.handle_raw_events(vec![RawInputEvent::Key(TerminalKey::new(
            KeyCode::Esc,
            KeyModifiers::NONE,
        ))]);
        assert!(state.copy_mode.as_ref().unwrap().selection.is_none());
        state.copy_mode.as_mut().unwrap().cursor.col = origin_col;
        let origin = cursor(&state);
        let input = state.handle_input_bytes(keys.as_bytes());
        reply(&mut state, &request_id(&input), text);
        let mode = state.copy_mode.as_ref().unwrap();
        assert!(!mode.vim_input.text_object, "{keys}");
        match expected {
            Some((start, end)) => {
                assert_eq!(cursor(&state), PaneTextPoint { col: end, ..origin });
                assert_eq!(
                    mode.selection,
                    Some(ClientCopySelection::Character {
                        anchor: PaneTextPoint {
                            col: start,
                            ..origin
                        }
                    }),
                    "{keys}"
                );
                assert_eq!(
                    state.selection.as_ref().unwrap().ordered_cells(),
                    ((origin.row, start), (origin.row, end)),
                    "{keys}"
                );
            }
            None => {
                assert_eq!(cursor(&state), origin, "{keys}");
                assert_eq!(mode.selection.is_some(), keys.starts_with('v'), "{keys}");
            }
        }
    }
}

#[test]
fn copy_vim_text_object_prefix_cancels_and_queues_like_other_pending_keys() {
    // Escape and unknown object keys drop the pending `i` without side effects.
    let mut state = visual();
    let origin = cursor(&state);
    state.handle_input_bytes(b"i");
    assert!(state.copy_mode.as_ref().unwrap().vim_input.text_object);
    state.handle_raw_events(vec![RawInputEvent::Key(TerminalKey::new(
        KeyCode::Esc,
        KeyModifiers::NONE,
    ))]);
    assert!(state.copy_mode.as_ref().unwrap().selection.is_some());
    let input = state.handle_input_bytes(b"ij");
    assert!(input.actions.is_empty());
    assert_eq!(cursor(&state), origin);
    state.handle_input_bytes(b"j");
    assert_eq!(cursor(&state).row, origin.row + 1);

    // Keys typed behind the row read wait for it, so `iwy` yanks the word.
    let mut state = visual();
    let input = state.handle_input_bytes(b"iwy");
    assert_eq!(state.mode, ClientShellMode::Copy);
    let actions = reply(&mut state, &request_id(&input), "ab cd");
    assert_eq!(state.mode, ClientShellMode::Terminal);
    assert!(actions.iter().any(|action| matches!(action,
        ClientShellAction::Endpoint { request, .. }
            if matches!(&request.method, Method::PaneSelectionRead(params)
                if params.anchor.col == 0 && params.cursor.col == 1)
    )));
}
