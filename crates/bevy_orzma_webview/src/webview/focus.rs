//! Webview keyboard focus: applies the GUI's focus requests at once, reports
//! them to the webview host, and mirrors the host's answers into
//! `FocusedWebview`.

use crate::webview::mount::{Webview, webview_of_mount};
use bevy::prelude::*;
use bevy_cef::prelude::FocusedWebview;
use bevy_orzmux::prelude::{OrzmuxConnection, OrzmuxWebviewEvent, PaneAction, RequestPaneAction};
use orzma_webview_host::prelude::{WebviewCommand, WebviewEvent};
use orzmux::prelude::{CommandSeq, OrzmuxCommand};

/// Asks for webview keyboard focus to move to the webview `target`, or to be
/// released when `target` is `None`.
///
/// The focus changes at once, and a target in an inactive pane also selects
/// that pane; the webview host confirms or corrects the change afterwards. A
/// request that would not change the focus, and a target that is not a
/// mounted webview, are ignored.
#[derive(Event, Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestWebviewFocus {
    target: Option<Entity>,
}

/// Applies focus requests and mirrors the host's focus into
/// `FocusedWebview`.
pub(crate) struct WebviewFocusPlugin;

impl RequestWebviewFocus {
    /// A request to focus the webview `target`, or to release webview focus
    /// when `target` is `None`.
    pub fn new(target: Option<Entity>) -> Self {
        Self { target }
    }

    /// The webview to focus, or `None` to release webview focus.
    pub fn target(&self) -> Option<Entity> {
        self.target
    }
}

impl Plugin for WebviewFocusPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FocusSync>()
            .add_observer(request_webview_focus.run_if(resource_exists::<OrzmuxConnection>))
            .add_observer(mirror_host_focus)
            .add_observer(release_removed_focus);
    }
}

/// Writes `next` into `focused` only when it differs, so change detection on
/// `FocusedWebview` fires exactly on a real change.
fn set_focused_webview(focused: &mut ResMut<FocusedWebview>, next: Option<Entity>) {
    if focused.0 != next {
        focused.0 = next;
    }
}

/// The sequence of the last `Focus` command the GUI sent, against which the
/// host's `FocusChanged` answers are reconciled.
#[derive(Resource, Default)]
struct FocusSync {
    last_sent: Option<CommandSeq>,
}

/// Applies a focus request that changes the focus: writes `FocusedWebview`,
/// asks for the target's pane to be selected, and sends the host a `Focus` of
/// the target's mount, remembering the command's sequence.
fn request_webview_focus(
    ev: On<RequestWebviewFocus>,
    mut commands: Commands,
    mut sync: ResMut<FocusSync>,
    mut focused: ResMut<FocusedWebview>,
    connection: Res<OrzmuxConnection>,
    webviews: Query<(&Webview, &ChildOf)>,
) {
    let target = ev.target();
    if focused.0 == target {
        return;
    }
    let mount = match target {
        Some(child) => {
            let Ok((view, parent)) = webviews.get(child) else {
                tracing::debug!(
                    ?child,
                    "focus request for an entity that is not a mounted webview dropped"
                );
                return;
            };
            commands.trigger(RequestPaneAction {
                action: PaneAction::Select(parent.parent()),
            });
            Some(view.mount())
        }
        None => None,
    };
    set_focused_webview(&mut focused, target);
    sync.last_sent = Some(
        connection
            .0
            .send(OrzmuxCommand::Webview(WebviewCommand::Focus { mount })),
    );
}

/// Mirrors a host `FocusChanged` into `FocusedWebview`, unless the backend
/// stamped it before it processed the last `Focus` the GUI sent. A focused
/// mount with no live webview mirrors as no focus.
fn mirror_host_focus(
    ev: On<OrzmuxWebviewEvent>,
    mut focused: ResMut<FocusedWebview>,
    sync: Res<FocusSync>,
    webviews: Query<(Entity, &Webview)>,
) {
    let WebviewEvent::FocusChanged { focused: mount } = ev.webview_event() else {
        return;
    };
    if sync.last_sent.is_some_and(|sent| ev.seq() < sent) {
        return;
    }
    let next = mount.and_then(|mount| webview_of_mount(&webviews, mount));
    set_focused_webview(&mut focused, next);
}

/// Releases the webview focus when the webview that holds it is removed,
/// whether the host ended its mount or its pane was despawned.
fn release_removed_focus(ev: On<Remove, Webview>, mut focused: ResMut<FocusedWebview>) {
    if focused.0 == Some(ev.entity) {
        set_focused_webview(&mut focused, None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::webview::mount::WebviewPlugin;
    use bevy::ecs::system::RunSystemOnce;
    use bevy_orzmux::prelude::{
        OrzmuxActivePaneChanged, OrzmuxClient, OrzmuxPane, OrzmuxPlugin, PaneRegistry,
    };
    use crossbeam_channel::Receiver;
    use orzma_vt::prelude::{InstanceId, PlacementSize};
    use orzma_webview_host::prelude::{HandleId, MountId, MountSpec};
    use orzmux::prelude::PaneId;

    /// The pane selections the focus observer asked for.
    #[derive(Resource, Default)]
    struct PaneSelections(Vec<PaneAction>);

    fn focus_app() -> (App, Receiver<(CommandSeq, OrzmuxCommand)>) {
        let (client, _events, commands) = OrzmuxClient::detached();
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins((WebviewPlugin, WebviewFocusPlugin))
            .init_resource::<Assets<Image>>()
            .init_resource::<FocusedWebview>()
            .init_resource::<PaneSelections>()
            .insert_resource(OrzmuxConnection(client))
            .add_observer(
                |ev: On<RequestPaneAction>, mut seen: ResMut<PaneSelections>| {
                    seen.0.push(ev.action);
                },
            );
        (app, commands)
    }

    fn spec() -> MountSpec {
        MountSpec::new(
            HandleId::from("h"),
            "orzma://h/index.html",
            PlacementSize { rows: 10, cols: 40 },
        )
    }

    fn host_event(app: &mut App, event: WebviewEvent<Entity>, seq: u64) {
        app.world_mut()
            .trigger(OrzmuxWebviewEvent::new(event, CommandSeq(seq)));
        app.world_mut().flush();
    }

    /// Spawns a pane entity and a webview of `mount` under it through the
    /// host's `Mounted`, returning `(pane, webview)`.
    fn mounted(app: &mut App, mount: MountId) -> (Entity, Entity) {
        let pane = app.world_mut().spawn_empty().id();
        host_event(
            app,
            WebviewEvent::Mounted {
                pane,
                mount,
                instance: InstanceId(1),
                spec: spec(),
            },
            0,
        );
        let mut webviews = app.world_mut().query::<(Entity, &Webview)>();
        let webview = webviews
            .iter(app.world())
            .find(|(_, view)| view.mount() == mount)
            .map(|(entity, _)| entity)
            .expect("the mount spawned a webview");
        (pane, webview)
    }

    fn sent_focus(
        commands: &Receiver<(CommandSeq, OrzmuxCommand)>,
    ) -> Vec<(CommandSeq, Option<MountId>)> {
        commands
            .try_iter()
            .filter_map(|(seq, command)| match command {
                OrzmuxCommand::Webview(WebviewCommand::Focus { mount }) => Some((seq, mount)),
                _ => None,
            })
            .collect()
    }

    fn request(app: &mut App, target: Option<Entity>) {
        app.world_mut().trigger(RequestWebviewFocus::new(target));
        app.world_mut().flush();
    }

    fn focused(app: &App) -> Option<Entity> {
        app.world().resource::<FocusedWebview>().0
    }

    /// Asserts that a focus request moves the focus at once, selects the
    /// target's pane, and reports the target's mount to the host.
    ///
    /// Case: the user clicks a page mounted in a pane.
    #[test]
    fn a_focus_request_applies_at_once_and_reports_the_mount() {
        let (mut app, commands) = focus_app();
        let (pane, webview) = mounted(&mut app, MountId::new(1));
        request(&mut app, Some(webview));
        assert_eq!(focused(&app), Some(webview));
        assert_eq!(
            app.world().resource::<PaneSelections>().0,
            vec![PaneAction::Select(pane)]
        );
        let sent = sent_focus(&commands);
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].1, Some(MountId::new(1)));
    }

    /// Asserts that a request that would not change the focus sends nothing.
    ///
    /// Case: the user clicks again into the page that already has focus.
    #[test]
    fn a_request_that_changes_nothing_sends_nothing() {
        let (mut app, commands) = focus_app();
        let (_, webview) = mounted(&mut app, MountId::new(1));
        app.world_mut().resource_mut::<FocusedWebview>().0 = Some(webview);
        request(&mut app, Some(webview));
        assert!(sent_focus(&commands).is_empty());
        assert!(app.world().resource::<PaneSelections>().0.is_empty());
    }

    /// Asserts that a release request clears the focus at once and reports
    /// no mount to the host, without selecting any pane.
    ///
    /// Case: the user presses the release-focus key while a page has focus.
    #[test]
    fn a_release_request_reports_no_mount() {
        let (mut app, commands) = focus_app();
        let (_, webview) = mounted(&mut app, MountId::new(1));
        app.world_mut().resource_mut::<FocusedWebview>().0 = Some(webview);
        request(&mut app, None);
        assert_eq!(focused(&app), None);
        assert_eq!(
            sent_focus(&commands)
                .into_iter()
                .map(|(_, mount)| mount)
                .collect::<Vec<_>>(),
            vec![None]
        );
        assert!(app.world().resource::<PaneSelections>().0.is_empty());
    }

    /// Asserts that a request naming an entity that is not a mounted webview
    /// changes nothing.
    ///
    /// Case: a click races the unmount of the page under the pointer.
    #[test]
    fn a_request_for_an_entity_that_is_not_a_webview_is_ignored() {
        let (mut app, commands) = focus_app();
        let stray = app.world_mut().spawn_empty().id();
        request(&mut app, Some(stray));
        assert_eq!(focused(&app), None);
        assert!(sent_focus(&commands).is_empty());
    }

    /// Asserts that a host focus older than the last request is ignored,
    /// while the answer to that request and every later host focus are
    /// mirrored.
    ///
    /// Case: the user presses the release-focus key while the page's program
    /// keeps focusing its page over the socket, once before the host handles
    /// the release and once after.
    #[test]
    fn a_host_focus_older_than_the_last_request_is_ignored() {
        let (mut app, commands) = focus_app();
        let (_, webview) = mounted(&mut app, MountId::new(1));
        app.world_mut().resource_mut::<FocusedWebview>().0 = Some(webview);
        request(&mut app, None);
        let (seq, _) = sent_focus(&commands)[0];

        host_event(
            &mut app,
            WebviewEvent::FocusChanged {
                focused: Some(MountId::new(1)),
            },
            seq.0 - 1,
        );
        assert_eq!(focused(&app), None);

        host_event(
            &mut app,
            WebviewEvent::FocusChanged { focused: None },
            seq.0,
        );
        assert_eq!(focused(&app), None);

        host_event(
            &mut app,
            WebviewEvent::FocusChanged {
                focused: Some(MountId::new(1)),
            },
            seq.0,
        );
        assert_eq!(focused(&app), Some(webview));
    }

    /// Asserts that a host focus is mirrored onto the webview of its mount.
    ///
    /// Case: a program focuses its page with the socket `focus` op while the
    /// GUI has sent no focus of its own.
    #[test]
    fn a_host_focus_is_mirrored_onto_the_webview_of_its_mount() {
        let (mut app, _commands) = focus_app();
        let (_, webview) = mounted(&mut app, MountId::new(1));
        host_event(
            &mut app,
            WebviewEvent::FocusChanged {
                focused: Some(MountId::new(1)),
            },
            0,
        );
        assert_eq!(focused(&app), Some(webview));
    }

    /// Asserts that a host focus on a mount with no live webview clears the
    /// focus.
    ///
    /// Case: the host's answer names a mount whose webview could not be
    /// spawned.
    #[test]
    fn a_host_focus_for_a_mount_without_a_webview_clears_the_focus() {
        let (mut app, _commands) = focus_app();
        let (_, webview) = mounted(&mut app, MountId::new(1));
        app.world_mut().resource_mut::<FocusedWebview>().0 = Some(webview);
        host_event(
            &mut app,
            WebviewEvent::FocusChanged {
                focused: Some(MountId::new(42)),
            },
            0,
        );
        assert_eq!(focused(&app), None);
    }

    /// Asserts that a focus the host reports right after a mount, in the
    /// same drain, lands on the new webview.
    ///
    /// Case: orzmd mounts its page and focuses it with the socket `focus` op
    /// in one burst of work.
    #[test]
    fn a_mount_then_focus_in_one_drain_focuses_the_new_webview() {
        let (mut app, _commands) = focus_app();
        let pane = app.world_mut().spawn_empty().id();
        app.world_mut()
            .run_system_once(move |mut commands: Commands| {
                commands.trigger(OrzmuxWebviewEvent::new(
                    WebviewEvent::Mounted {
                        pane,
                        mount: MountId::new(1),
                        instance: InstanceId(1),
                        spec: spec(),
                    },
                    CommandSeq(0),
                ));
                commands.trigger(OrzmuxWebviewEvent::new(
                    WebviewEvent::FocusChanged {
                        focused: Some(MountId::new(1)),
                    },
                    CommandSeq(0),
                ));
            })
            .expect("the drain system runs");
        let mut webviews = app.world_mut().query::<(Entity, &Webview)>();
        let webview = webviews
            .iter(app.world())
            .map(|(entity, _)| entity)
            .next()
            .expect("the mount spawned a webview");
        assert_eq!(focused(&app), Some(webview));
    }

    /// Asserts that unmounting the focused webview releases the focus at
    /// once, before any answer from the host.
    ///
    /// Case: a pane closes while one of its pages holds keyboard focus.
    #[test]
    fn an_unmount_of_the_focused_webview_releases_focus_at_once() {
        let (mut app, _commands) = focus_app();
        let (_, webview) = mounted(&mut app, MountId::new(1));
        app.world_mut().resource_mut::<FocusedWebview>().0 = Some(webview);
        host_event(
            &mut app,
            WebviewEvent::Unmounted {
                mounts: vec![MountId::new(1)],
            },
            0,
        );
        assert_eq!(focused(&app), None);
    }

    /// Asserts that despawning the pane of the focused webview releases the
    /// focus without an unmount from the host.
    ///
    /// Case: a pane's shell exits while one of its pages holds keyboard
    /// focus, and the GUI removes the pane before the host's unmount of the
    /// page reaches it.
    #[test]
    fn a_despawned_pane_releases_the_focus_of_its_webview() {
        let (mut app, _commands) = focus_app();
        let (pane, webview) = mounted(&mut app, MountId::new(1));
        app.world_mut().resource_mut::<FocusedWebview>().0 = Some(webview);
        app.world_mut().entity_mut(pane).despawn();
        assert_eq!(focused(&app), None);
    }

    /// Asserts that a focus request made in one system set has, by the time
    /// a later set runs in the same update, focused the page and made its
    /// pane the active pane.
    ///
    /// Case: the user clicks a page in the inactive pane and types at once.
    #[test]
    fn a_click_in_an_inactive_pane_activates_it_before_key_dispatch() {
        #[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
        enum Phase {
            Click,
            Keys,
        }
        #[derive(Component)]
        struct Active;
        #[derive(Resource, Default)]
        struct Clicked(Option<Entity>);
        #[derive(Resource, Default)]
        struct SeenAtKeys(Option<(Option<Entity>, Vec<Entity>)>);

        let (client, _events, _commands) = OrzmuxClient::detached();
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins((OrzmuxPlugin, WebviewPlugin, WebviewFocusPlugin))
            .init_resource::<Assets<Image>>()
            .init_resource::<FocusedWebview>()
            .init_resource::<Clicked>()
            .init_resource::<SeenAtKeys>()
            .insert_resource(OrzmuxConnection(client))
            .configure_sets(Update, (Phase::Click, Phase::Keys).chain())
            .add_systems(
                Update,
                (|mut commands: Commands, mut clicked: ResMut<Clicked>| {
                    if let Some(child) = clicked.0.take() {
                        commands.trigger(RequestWebviewFocus::new(Some(child)));
                    }
                })
                .in_set(Phase::Click),
            )
            .add_systems(
                Update,
                (|mut seen: ResMut<SeenAtKeys>,
                  focused: Res<FocusedWebview>,
                  active: Query<Entity, With<Active>>| {
                    seen.0 = Some((focused.0, active.iter().collect()));
                })
                .in_set(Phase::Keys),
            )
            .add_observer(|ev: On<OrzmuxActivePaneChanged>, mut commands: Commands| {
                if let Some(previous) = ev.previous {
                    commands.entity(previous).remove::<Active>();
                }
                if let Some(current) = ev.current {
                    commands.entity(current).insert(Active);
                }
            });
        let left = app.world_mut().spawn((OrzmuxPane(PaneId(1)), Active)).id();
        let right = app.world_mut().spawn(OrzmuxPane(PaneId(2))).id();
        {
            let mut registry = app.world_mut().resource_mut::<PaneRegistry>();
            registry.panes.insert(PaneId(1), left);
            registry.panes.insert(PaneId(2), right);
            registry.applied_active = Some(PaneId(1));
        }
        host_event(
            &mut app,
            WebviewEvent::Mounted {
                pane: right,
                mount: MountId::new(1),
                instance: InstanceId(1),
                spec: spec(),
            },
            0,
        );
        let mut webviews = app.world_mut().query::<(Entity, &Webview)>();
        let webview = webviews
            .iter(app.world())
            .map(|(entity, _)| entity)
            .next()
            .expect("the mount spawned a webview");
        app.update();

        app.world_mut().resource_mut::<Clicked>().0 = Some(webview);
        app.update();

        assert_eq!(
            app.world().resource::<SeenAtKeys>().0,
            Some((Some(webview), vec![right]))
        );
    }
}
