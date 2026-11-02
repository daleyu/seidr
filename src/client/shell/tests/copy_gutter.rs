use super::*;

fn numbered_state(width: u16, height: u16) -> ClientShellState {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.config.copy_on_select = false;
    state.set_snapshot(Box::new(snapshot()));
    let mut next = surface();
    next.frame = FrameData::from_ratatui_buffer(
        &Buffer::with_lines(vec!["abcdefghijklmnopqrstuvwxyz"; usize::from(height)]),
        Some(crate::protocol::CursorState {
            x: 2,
            y: height / 2,
            visible: true,
            shape: 2,
        }),
    );
    next.panes[0].inner_rect = SurfaceRect {
        x: 0,
        y: 0,
        width,
        height,
    };
    next.panes[0].rect = next.panes[0].inner_rect;
    next.panes[0].scroll = Some(crate::protocol::PaneSurfaceScrollMetrics {
        offset_from_bottom: 20,
        max_offset_from_bottom: 40,
        viewport_rows: u64::from(height),
    });
    state.set_pane_surface(next);
    state.compose(106, 30).expect("initial frame");
    state.handle_input_bytes(b"\x02[");
    assert_eq!(state.mode, ClientShellMode::Copy);
    state
}

fn pane_rows(state: &ClientShellState, frame: &FrameData) -> Vec<String> {
    let rect = state.hits.panes[0].inner_rect;
    (rect.y..rect.bottom())
        .map(|y| {
            let start = usize::from(y) * usize::from(frame.width) + usize::from(rect.x);
            frame.cells[start..start + usize::from(rect.width)]
                .iter()
                .map(|cell| cell.symbol.as_str())
                .collect()
        })
        .collect()
}

#[test]
fn copy_gutter_tracks_counted_motion_selection_and_exit_without_resizing() {
    let mut state = numbered_state(20, 7);
    let before = state.pane_surface.as_ref().unwrap().clone();
    let frame = state.compose(106, 30).unwrap();
    let rows = pane_rows(&state, &frame);
    assert_eq!(
        &rows[..],
        &[
            "3 abcdefghijklmnopqr",
            "2 abcdefghijklmnopqr",
            "1 abcdefghijklmnopqr",
            "0 abcdefghijklmnopqr",
            "1 abcdefghijklmnopqr",
            "2 abcdefghijklmnopqr",
            "3 abcdefghijklmnopqr",
        ]
    );
    let input = state.handle_input_bytes(b"v2k");
    assert!(input.actions.is_empty());
    assert!(!input.resize);
    let frame = state.compose(106, 30).unwrap();
    let rows = pane_rows(&state, &frame);
    assert!(rows[0].starts_with("1 "));
    assert!(rows[1].starts_with("0 "));
    assert!(rows[3].starts_with("2 "));
    assert_eq!(
        state.selection.as_ref().unwrap().ordered_cells(),
        ((21, 2), (23, 2))
    );
    assert_eq!(state.pane_surface.as_ref().unwrap(), &before);

    let copy = state.handle_input_bytes(b"y");
    assert!(copy.actions.iter().any(|action| matches!(action,
        ClientShellAction::Endpoint { request, .. }
        if matches!(&request.method, crate::api::schema::Method::PaneSelectionRead(params)
            if params.anchor.col == 2 && params.cursor.col == 2)
    )));
    let frame = state.compose(106, 30).unwrap();
    assert!(state.copy_gutter.is_none());
    assert_eq!(pane_rows(&state, &frame)[0], "abcdefghijklmnopqrst");
}

#[test]
fn copy_gutter_pans_to_keep_cursor_visible_and_preserves_cell_effects() {
    let mut state = numbered_state(20, 7);
    let next = state.pane_surface.as_mut().unwrap();
    next.frame.hyperlinks.push("https://example.com".into());
    next.frame.cells[3].hyperlink = Some(0);
    state.copy_mode.as_mut().unwrap().cursor.col = 19;
    let frame = state.compose(106, 30).unwrap();
    let gutter = state.copy_gutter.unwrap();
    assert_eq!(gutter.left_col, 2);
    assert_eq!(pane_rows(&state, &frame)[0], "3 cdefghijklmnopqrst");
    let start = usize::from(gutter.rect.y) * usize::from(frame.width) + usize::from(gutter.rect.x);
    assert_eq!(frame.cells[start + 3].hyperlink, Some(0));
    assert_eq!(frame.cells[start].hyperlink, None);
    let cursor_index = start + 3 * usize::from(frame.width) + 19;
    assert_eq!(frame.cells[cursor_index].symbol, "t");
    assert_eq!(
        frame.cells[cursor_index].bg,
        crate::protocol::color_to_u32(state.config.palette.accent)
    );
    let hit = &state.hits.panes[0];
    assert_eq!(state.pane_source_column(hit, hit.inner_rect.x + 2), 2);
    assert_eq!(state.pane_source_column(hit, hit.inner_rect.x + 19), 19);
}

#[test]
fn copy_gutter_mouse_drag_uses_terminal_columns_and_ignores_number_clicks() {
    let mut state = numbered_state(20, 7);
    state.compose(106, 30).unwrap();
    let rect = state.hits.panes[0].inner_rect;
    let mouse = |kind, col| {
        RawInputEvent::Mouse(MouseEvent {
            kind,
            column: rect.x + col,
            row: rect.y + 1,
            modifiers: KeyModifiers::empty(),
        })
    };
    state.handle_raw_events(vec![mouse(MouseEventKind::Down(MouseButton::Left), 0)]);
    assert!(state.selection.is_none());
    state.handle_raw_events(vec![mouse(MouseEventKind::Down(MouseButton::Left), 3)]);
    state.handle_raw_events(vec![mouse(MouseEventKind::Drag(MouseButton::Left), 6)]);
    state.handle_raw_events(vec![mouse(MouseEventKind::Up(MouseButton::Left), 6)]);
    assert_eq!(
        state.selection.as_ref().unwrap().ordered_cells(),
        ((21, 1), (21, 4))
    );
    state.handle_raw_events(vec![mouse(MouseEventKind::Down(MouseButton::Left), 0)]);
    assert_eq!(
        state.selection.as_ref().unwrap().ordered_cells(),
        ((21, 1), (21, 4))
    );
}

#[test]
fn copy_gutter_clips_wide_characters_at_both_edges() {
    let mut state = numbered_state(10, 7);
    let mut next = state.pane_surface.as_ref().unwrap().clone();
    next.frame = FrameData::from_ratatui_buffer(&Buffer::with_lines(vec!["界bcdef界h"; 7]), None);
    state.set_pane_surface(next);
    let frame = state.compose(106, 30).unwrap();
    let gutter = state.copy_gutter.unwrap();
    let start = usize::from(gutter.rect.y) * usize::from(frame.width) + usize::from(gutter.rect.x);
    // The right-hand glyph would straddle the clipped edge.
    assert_eq!(frame.cells[start + 9].symbol, " ");
    // A cursor near the right edge pans the text so it begins on a wide tail.
    state.copy_mode.as_mut().unwrap().cursor.col = 7;
    let frame = state.compose(106, 30).unwrap();
    assert_eq!(state.copy_gutter.unwrap().left_col, 1);
    assert_eq!(frame.cells[start + 2].symbol, " ");
    assert!(!frame.cells[start + 2].skip);
    assert_eq!(frame.cells[start + 9].symbol, " ");
}

#[test]
#[ignore = "manual copy gutter composition scaling profile"]
fn copy_gutter_render_scale_profile() {
    for count in [1, 15] {
        let mut state = numbered_state(20, 7);
        state.mode = ClientShellMode::Terminal;
        state.copy_mode = None;
        let mut next = surface();
        next.frame =
            FrameData::from_ratatui_buffer(&Buffer::with_lines(vec!["x".repeat(120); 30]), None);
        let template = next.panes[0].clone();
        next.panes.clear();
        for index in 0..count {
            let mut pane = template.clone();
            pane.pane_id = format!("pane_{}", index + 1);
            pane.inner_rect = SurfaceRect {
                x: if count == 1 { 0 } else { (index % 5) * 24 },
                y: if count == 1 { 0 } else { (index / 5) * 10 },
                width: if count == 1 { 120 } else { 24 },
                height: if count == 1 { 30 } else { 10 },
            };
            pane.rect = pane.inner_rect;
            pane.scroll = Some(crate::protocol::PaneSurfaceScrollMetrics {
                offset_from_bottom: 20,
                max_offset_from_bottom: 40,
                viewport_rows: u64::from(pane.inner_rect.height),
            });
            next.panes.push(pane);
        }
        state.set_pane_surface(next);
        state.compose(146, 32).unwrap();
        for active in [false, true] {
            if active {
                assert!(state.enter_copy_mode(&mut ClientShellInput::default()));
            }
            let mut samples = Vec::new();
            for _ in 0..200 {
                let started = std::time::Instant::now();
                std::hint::black_box(state.compose(146, 32).unwrap());
                samples.push(started.elapsed().as_micros());
            }
            samples.sort_unstable();
            eprintln!(
                "copy-gutter panes={count} active={active} median_us={} p95_us={}",
                samples[100], samples[190]
            );
            assert!(state.pending_requests.is_empty());
        }
    }
}

#[test]
fn copy_gutter_handles_multi_digit_distances_and_narrow_or_stale_surfaces() {
    let mut state = numbered_state(20, 23);
    let frame = state.compose(106, 30).unwrap();
    assert_eq!(state.copy_gutter.unwrap().width, 3);
    let rows = pane_rows(&state, &frame);
    assert!(rows[0].starts_with("11 "));
    assert!(rows[11].starts_with(" 0 "));
    assert!(rows[22].starts_with("11 "));

    state.copy_mode.as_mut().unwrap().offset_from_bottom += 1;
    let frame = state.compose(106, 30).unwrap();
    assert!(state.copy_gutter.is_none());
    assert_eq!(pane_rows(&state, &frame)[0], "abcdefghijklmnopqrst");
    let mut narrow = numbered_state(2, 7);
    narrow.compose(106, 30).unwrap();
    assert!(narrow.copy_gutter.is_none());
}
