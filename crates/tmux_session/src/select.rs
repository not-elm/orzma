//! Choosing which tmux session to attach to at startup.

use tmux_control::SessionInfo;

/// The session to connect to when attaching.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttachTarget {
    /// Attach to an existing session by name.
    Attach(String),
    /// No suitable session exists; create a fresh one.
    CreateNew,
}

/// Chooses which session to attach to from a `list-sessions` snapshot.
///
/// Prefers an already-attached session (someone is using it); otherwise the
/// most-recently-created (highest `created`, tie-broken by highest id).
/// Returns [`AttachTarget::CreateNew`] when `sessions` is empty. tmux's
/// `SessionInfo` exposes no last-activity field, so creation time is the
/// best available recency proxy.
pub fn select_attach_target(sessions: &[SessionInfo]) -> AttachTarget {
    match sessions.iter().max_by(|a, b| {
        a.attached
            .cmp(&b.attached)
            .then(a.created.cmp(&b.created))
            .then(a.id.0.cmp(&b.id.0))
    }) {
        Some(session) => AttachTarget::Attach(session.name.clone()),
        None => AttachTarget::CreateNew,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tmux_control::SessionId;

    fn session(id: u32, name: &str, attached: bool, created: u64) -> SessionInfo {
        SessionInfo {
            id: SessionId(id),
            name: name.to_string(),
            windows: 1,
            attached,
            created,
        }
    }

    #[test]
    fn empty_creates_new() {
        assert_eq!(select_attach_target(&[]), AttachTarget::CreateNew);
    }

    #[test]
    fn single_session_is_chosen() {
        let s = vec![session(0, "main", false, 10)];
        assert_eq!(
            select_attach_target(&s),
            AttachTarget::Attach("main".to_string())
        );
    }

    #[test]
    fn attached_beats_more_recent_unattached() {
        let s = vec![
            session(0, "old-attached", true, 10),
            session(1, "new-detached", false, 99),
        ];
        assert_eq!(
            select_attach_target(&s),
            AttachTarget::Attach("old-attached".to_string())
        );
    }

    #[test]
    fn among_unattached_most_recent_created_wins() {
        let s = vec![
            session(0, "older", false, 10),
            session(1, "newer", false, 20),
        ];
        assert_eq!(
            select_attach_target(&s),
            AttachTarget::Attach("newer".to_string())
        );
    }

    #[test]
    fn created_tie_broken_by_highest_id() {
        let s = vec![
            session(0, "a", false, 10),
            session(2, "c", false, 10),
            session(1, "b", false, 10),
        ];
        assert_eq!(
            select_attach_target(&s),
            AttachTarget::Attach("c".to_string())
        );
    }
}
