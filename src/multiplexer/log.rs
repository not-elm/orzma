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
    let mut sids: Vec<_> = svc.sessions.iter().collect();
    sids.sort_by(|a, b| a.0.as_ref().cmp(b.0.as_ref()));

    let mut out = String::from("Sessions:\n");
    for (sid, session) in sids {
        out.push_str(&format!("  Session({sid}) \"{}\"\n", session.name));
        let mut wids: Vec<_> = session.linked_windows.iter().collect();
        wids.sort_by(|a, b| a.as_ref().cmp(b.as_ref()));
        for wid in wids {
            let Some(window) = svc.windows.get(wid) else {
                continue;
            };
            let active = window.active_pane.to_string();
            let dims = window
                .dimensions
                .as_ref()
                .map(|d| format!("[{}x{}]", d.cols, d.rows))
                .unwrap_or_else(|| "[?]".into());
            out.push_str(&format!(
                "    Window({wid}) \"{}\" {dims} active={active}\n",
                window.name
            ));
            let mut pids: Vec<_> = window.pane_ids().collect();
            pids.sort_by(|a, b| a.as_ref().cmp(b.as_ref()));
            for pid in pids {
                let Ok(pane) = window.pane(pid) else { continue };
                let pa = pane.active_activity.to_string();
                out.push_str(&format!("      Pane({pid}) active={pa}\n"));
                let mut aids: Vec<_> = pane.activity_ids().collect();
                aids.sort_by(|a, b| a.as_ref().cmp(b.as_ref()));
                for aid in aids {
                    let kind = match pane.activity(aid).map(|a| &a.kind) {
                        Some(k) => format!("{k:?}"),
                        None => "?".into(),
                    };
                    out.push_str(&format!("        Activity({aid}) {kind}\n"));
                }
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
    fn render_tree_includes_session_window_pane_activity() {
        let mut svc = MultiplexerService::default();
        let sid = svc.create_session(Some("default".into()));
        svc.create_window(Some(&sid), Some("main".into())).unwrap();

        let output = render_tree(&svc);
        assert!(output.starts_with("Sessions:\n"));
        assert!(output.contains("Session("));
        assert!(output.contains("\"default\""));
        assert!(output.contains("Window("));
        assert!(output.contains("\"main\""));
        assert!(output.contains("Pane("));
        assert!(output.contains("Activity("));
    }

    #[test]
    fn render_tree_is_deterministic_across_runs() {
        let mut svc = MultiplexerService::default();
        let sid = svc.create_session(Some("default".into()));
        svc.create_window(Some(&sid), Some("a".into())).unwrap();
        svc.create_window(Some(&sid), Some("b".into())).unwrap();

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
            let _sid = mux.create_session(Some("plugin-test".into()));
        }
        app.update();
    }
}
