//! Recent screen preview for the agent navigator's selected row.
//!
//! Reads run only while the agent navigator is open, one request at a time, and
//! never raise endpoint notices: a missing preview must not interrupt browsing.

use super::*;

const PREVIEW_REFRESH: std::time::Duration = std::time::Duration::from_secs(1);

impl ClientShellState {
    pub(crate) fn tick_navigator_preview(&mut self, now: std::time::Instant) -> ClientShellInput {
        let mut outcome = ClientShellInput::default();
        let Some(ClientShellOverlay::Navigator(navigator)) = self.overlay.as_ref() else {
            return outcome;
        };
        if navigator.scope != ClientNavigatorScope::Agents || navigator.preview.disabled {
            return outcome;
        }
        let rows =
            render::client_navigator_rows(&self.endpoints, &self.active_endpoint_id, navigator);
        // Inactive endpoints reject reads until their surface is active, so only
        // agents on the displayed endpoint can be previewed.
        let pane_id = match super::aggregate_navigation::selected_navigator_target(&rows, navigator)
        {
            Some(ClientNavigatorTarget::Pane {
                endpoint_id,
                pane_id,
            }) if endpoint_id == self.active_endpoint_id => Some(pane_id),
            _ => None,
        };
        // Read intent does not cross the wire, so a recent-history read would be
        // interactive and could scroll an alternate-screen agent to capture it.
        // The visible screen is side-effect free and matches what a jump shows.
        let method = pane_id.as_ref().map(|pane_id| {
            crate::api::schema::Method::PaneRead(crate::api::schema::PaneReadParams {
                pane_id: pane_id.clone(),
                source: crate::api::schema::ReadSource::Visible,
                lines: None,
                format: crate::api::schema::ReadFormat::Text,
                strip_ansi: true,
                intent: crate::api::schema::ReadIntent::Passive,
            })
        });
        let readable = method.as_ref().is_some_and(|method| {
            self.endpoint_is_online(&self.active_endpoint_id)
                && self.supports_endpoint_method(method)
        });
        let Some(ClientShellOverlay::Navigator(navigator)) = self.overlay.as_mut() else {
            return outcome;
        };
        let preview = &mut navigator.preview;
        if preview.pane_id != pane_id {
            preview.pane_id = pane_id;
            preview.text = None;
            preview.unavailable = !readable;
            preview.fetched_at = None;
            outcome.repaint = true;
        }
        let (Some(method), true) = (method, readable) else {
            return outcome;
        };
        if preview.request_id.is_some()
            || preview.fetched_at.is_some_and(|fetched_at| {
                now.saturating_duration_since(fetched_at) < PREVIEW_REFRESH
            })
        {
            return outcome;
        }
        let action_index = outcome.actions.len();
        if self.push_endpoint_method_with_kind(
            method,
            PendingEndpointKind::NavigatorPreview,
            &mut outcome,
        ) {
            let request_id = match outcome.actions.get(action_index) {
                Some(ClientShellAction::Endpoint { request, .. }) => Some(request.id.clone()),
                _ => None,
            };
            if let Some(ClientShellOverlay::Navigator(navigator)) = self.overlay.as_mut() {
                navigator.preview.request_id = request_id;
            }
        }
        outcome
    }

    pub(super) fn complete_navigator_preview(
        &mut self,
        request_id: &str,
        result: Result<crate::api::schema::ResponseResult, ClientShellEndpointError>,
    ) -> (bool, Vec<ClientShellAction>) {
        let Some(ClientShellOverlay::Navigator(navigator)) = self.overlay.as_mut() else {
            return (false, Vec::new());
        };
        let preview = &mut navigator.preview;
        if preview.request_id.as_deref() != Some(request_id) {
            return (false, Vec::new());
        }
        preview.request_id = None;
        let repaint = match result {
            Ok(crate::api::schema::ResponseResult::PaneRead { read })
                if preview.pane_id.as_deref() == Some(read.pane_id.as_str()) =>
            {
                preview.fetched_at = Some(std::time::Instant::now());
                preview.unavailable = false;
                let changed = preview.text.as_deref() != Some(read.text.as_str());
                preview.text = Some(read.text);
                changed
            }
            // The selection moved while this read was in flight; the next tick
            // requests the new pane without waiting for the refresh interval.
            Ok(_) => false,
            Err(error) => {
                preview.fetched_at = Some(std::time::Instant::now());
                if error.code.as_deref() == Some("unsupported_endpoint_command") {
                    preview.disabled = true;
                }
                let changed = !preview.unavailable && preview.text.is_none();
                preview.unavailable |= preview.text.is_none();
                changed
            }
        };
        (repaint, Vec::new())
    }
}

/// The last non-blank lines of a preview, newest at the bottom.
pub(super) fn navigator_preview_lines(text: &str, max_lines: usize) -> Vec<&str> {
    let lines = text.lines().collect::<Vec<_>>();
    let end = lines
        .iter()
        .rposition(|line| !line.trim().is_empty())
        .map_or(0, |index| index + 1);
    lines[end.saturating_sub(max_lines)..end].to_vec()
}

#[cfg(test)]
mod tests {
    use super::navigator_preview_lines;

    #[test]
    fn preview_lines_drop_trailing_blank_rows_and_keep_the_newest_output() {
        let text = "one\ntwo\n\nthree\n   \n\n";
        assert_eq!(navigator_preview_lines(text, 2), vec!["", "three"]);
        assert_eq!(
            navigator_preview_lines(text, 10),
            vec!["one", "two", "", "three"]
        );
        assert!(navigator_preview_lines("\n\n", 4).is_empty());
    }
}
