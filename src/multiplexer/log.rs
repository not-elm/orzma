//! `log_layout_changes` system + `render_tree` formatter.
//! `OzmuxLayoutLogPlugin` registers the system in `Update`.

use crate::multiplexer::Multiplexer;
use bevy::prelude::*;
use ozmux_multiplexer::MultiplexerService;

/// Bevy Plugin that registers `log_layout_changes` in the `Update` schedule
/// behind `resource_changed::<Multiplexer>` so it only fires on layout change.
pub struct OzmuxLayoutLogPlugin;

impl Plugin for OzmuxLayoutLogPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            log_layout_changes.run_if(resource_changed::<Multiplexer>),
        );
    }
}

fn log_layout_changes(mux: Res<Multiplexer>) {
    tracing::info!(target: "ozmux_gui::layout", "\n{}", render_tree(&mux));
}

fn render_tree(svc: &MultiplexerService) -> String {
    let mut sids: Vec<_> = svc.sessions.keys().copied().collect();
    sids.sort();

    let mut out = String::from("Sessions:\n");
    for sid in sids {
        let Some(session) = svc.sessions.get(&sid) else {
            continue;
        };
        let dims = session
            .dimensions
            .as_ref()
            .map(|d| format!("[{}x{}]", d.cols, d.rows))
            .unwrap_or_else(|| "[?]".into());
        let active = session.active_pane.to_string();
        out.push_str(&format!(
            "  Session({sid}) \"{}\" {dims} active_pane={active}\n",
            session.name
        ));
        let mut pids: Vec<_> = session.pane_ids().collect();
        pids.sort_by(|a, b| a.as_ref().cmp(b.as_ref()));
        for pid in pids {
            let Ok(pane) = session.pane(pid) else {
                continue;
            };
            let pa = pane.active_activity.to_string();
            out.push_str(&format!("    Pane({pid}) active={pa}\n"));
            let mut aids: Vec<_> = pane.activity_ids().collect();
            aids.sort_by(|a, b| a.as_ref().cmp(b.as_ref()));
            for aid in aids {
                let kind = match pane.activity(aid).map(|a| &a.kind) {
                    Some(k) => format!("{k:?}"),
                    None => "?".into(),
                };
                out.push_str(&format!("      Activity({aid}) {kind}\n"));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_tree_empty_service_lists_no_sessions() {
        let svc = MultiplexerService::default();
        assert_eq!(render_tree(&svc), "Sessions:\n");
    }

    #[test]
    fn render_tree_includes_session_pane_activity() {
        let mut svc = MultiplexerService::default();
        let (_sid, _, _) = svc.create_session(Some("default".into()));

        let output = render_tree(&svc);
        assert!(output.starts_with("Sessions:\n"));
        assert!(output.contains("Session("));
        assert!(output.contains("\"default\""));
        assert!(output.contains("Pane("));
        assert!(output.contains("Activity("));
    }

    #[test]
    fn render_tree_is_deterministic_across_runs() {
        let mut svc = MultiplexerService::default();
        let _ = svc.create_session(Some("a".into()));
        let _ = svc.create_session(Some("b".into()));

        let first = render_tree(&svc);
        let second = render_tree(&svc);
        assert_eq!(first, second);
    }

    #[test]
    fn log_plugin_registers_system_without_panic() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(crate::multiplexer::OzmuxMultiplexerPlugin)
            .add_plugins(OzmuxLayoutLogPlugin);
        app.update();

        {
            let mut mux = app
                .world_mut()
                .resource_mut::<crate::multiplexer::Multiplexer>();
            let _ = mux.create_session(Some("plugin-test".into()));
        }
        app.update();
    }
}
