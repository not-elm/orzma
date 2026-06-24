//! ozmux-owned rename prompt for tmux's `command-prompt`-wrapped `rename-window`
//! / `rename-session` bindings, which a `-CC` control client cannot render.
//! `forward_keys_to_tmux` detects such a binding and inserts `RenamePrompt`
//! instead of forwarding it; this prompt owns the keyboard, pre-fills the
//! current name, and on submit sends a freshly-rebuilt, safely-quoted rename
//! command. The recognizer is `RenameKind::parse`.

use crate::font::TerminalUiFont;
use crate::theme;
use bevy::app::{App, Plugin};
use bevy::ecs::message::MessageReader;
use bevy::ecs::schedule::common_conditions::{
    resource_exists, resource_exists_and_changed, resource_removed,
};
use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::prelude::*;
use ozmux_tmux::{RenameSession, RenameWindow, SessionId, TmuxClient, TmuxCommand, WindowId};

const RENAME_Z: i32 = 340;

/// Registers the rename-prompt input system and the show/hide render systems.
pub(crate) struct RenamePromptPlugin;

impl Plugin for RenamePromptPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_rename_ui)
            .add_systems(
                Update,
                handle_rename_input
                    .after(crate::input::InputPhase::FocusedKey)
                    .run_if(resource_exists::<RenamePrompt>),
            )
            .add_systems(
                PostUpdate,
                (
                    hide_rename_ui.run_if(resource_removed::<RenamePrompt>),
                    show_rename_ui.run_if(resource_exists_and_changed::<RenamePrompt>),
                ),
            );
    }
}

/// What is being renamed: the captured target id plus its current name. One
/// enum so an invalid kind/id pairing is unrepresentable.
pub(crate) enum RenameSubject {
    /// A window, targeted by `@id`.
    Window {
        /// tmux window id captured at prompt-open.
        id: WindowId,
        /// The window's name at prompt-open (used to pre-fill the field).
        current_name: String,
    },
    /// A session, targeted by `$id`.
    Session {
        /// tmux session id captured at prompt-open.
        id: SessionId,
        /// The session's name at prompt-open (used to pre-fill the field).
        current_name: String,
    },
}

impl RenameSubject {
    fn current_name(&self) -> &str {
        match self {
            RenameSubject::Window { current_name, .. }
            | RenameSubject::Session { current_name, .. } => current_name,
        }
    }

    /// The prompt bar's leading label for this subject.
    fn label(&self) -> &'static str {
        match self {
            RenameSubject::Window { .. } => "Rename window: ",
            RenameSubject::Session { .. } => "Rename session: ",
        }
    }
}

/// The active rename prompt. Present as a resource only while editing; its
/// existence owns the keyboard like the confirm prompt and the session picker.
#[derive(Resource)]
pub(crate) struct RenamePrompt {
    /// What is being renamed.
    subject: RenameSubject,
    /// The edit buffer, pre-filled with the subject's current name.
    text: String,
}

impl RenamePrompt {
    /// Opens a prompt for `subject`, pre-filling the edit buffer with its
    /// current name.
    pub(crate) fn new(subject: RenameSubject) -> Self {
        let text = subject.current_name().to_string();
        Self { subject, text }
    }

    /// Applies one key: Enter submits, Escape cancels, Backspace deletes the
    /// last char, a character is appended. Other keys are ignored.
    fn apply_key(&mut self, key: &Key) -> RenameStep {
        match key {
            Key::Escape => RenameStep::Cancel,
            Key::Enter => RenameStep::Submit,
            Key::Backspace => {
                self.text.pop();
                RenameStep::Continue
            }
            Key::Character(s) => {
                self.text.push_str(s);
                RenameStep::Continue
            }
            _ => RenameStep::Continue,
        }
    }

    /// Builds the tmux command sent on submit from the subject and typed text.
    fn submit_command(&self) -> String {
        match &self.subject {
            RenameSubject::Window { id, .. } => RenameWindow {
                id: *id,
                name: &self.text,
            }
            .into_raw_command(),
            RenameSubject::Session { id, .. } => RenameSession {
                id: *id,
                name: &self.text,
            }
            .into_raw_command(),
        }
    }
}

/// The kind of rename a recognized binding performs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RenameKind {
    /// `rename-window` / `renamew`.
    Window,
    /// `rename-session` / `rename`.
    Session,
}

impl RenameKind {
    /// Recognizes a default-compatible single-input `command-prompt` rename
    /// binding, returning its [`RenameKind`], or `None` for anything ozmux
    /// should forward verbatim (other command-prompts, multi-input prompts,
    /// decorated templates, and non-`command-prompt` commands).
    ///
    /// Recognition is by command shape, not substring: the first token must be
    /// `command-prompt`; the inner template (a `{ ... }` brace body or a quoted
    /// template argument) must be exactly `<rename-verb> [--] <placeholder>`
    /// where the verb is `rename-window`/`renamew`/`rename-session`/`rename`
    /// and the placeholder is `%%` or `%1`. Multi-input prompts are rejected by
    /// arity (any `%2`..`%9` reference), which is robust against the `-l`
    /// literal flag and commas embedded in a single quoted prompt.
    pub(crate) fn parse(command: &str) -> Option<Self> {
        let tokens = tokenize(command);
        if tokens.first().map(String::as_str) != Some("command-prompt") {
            return None;
        }
        let inner = command_prompt_inner(&tokens)?;
        let inner_tokens = tokenize(&inner);
        if inner_tokens.iter().any(|t| has_high_arity_placeholder(t)) {
            return None;
        }
        rename_kind_of(&inner_tokens)
    }
}

/// The effect of one key on an open rename prompt.
#[derive(Debug, PartialEq, Eq)]
enum RenameStep {
    Continue,
    Submit,
    Cancel,
}

#[derive(Component)]
struct RenameBar;

fn spawn_rename_ui(mut commands: Commands, ui_font: Option<Res<TerminalUiFont>>) {
    let font = ui_font.as_deref().map(|f| f.0.clone()).unwrap_or_default();
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                bottom: Val::Px(0.0),
                width: Val::Percent(100.0),
                height: Val::Auto,
                display: Display::None,
                padding: UiRect::axes(Val::Px(6.0), Val::Px(3.0)),
                ..default()
            },
            BackgroundColor(theme::SELECTION),
            GlobalZIndex(RENAME_Z),
            RenameBar,
        ))
        .with_children(|parent| {
            parent.spawn((
                Text::new(""),
                TextFont {
                    font,
                    font_size: theme::UI_FONT_SIZE,
                    ..default()
                },
                TextColor(theme::SELECTION_FG),
            ));
        });
}

fn hide_rename_ui(mut bar: Query<&mut Node, With<RenameBar>>) {
    if let Ok(mut node) = bar.single_mut() {
        node.display = Display::None;
    }
}

fn show_rename_ui(
    mut bar: Query<&mut Node, With<RenameBar>>,
    mut texts: Query<&mut Text>,
    prompt: Res<RenamePrompt>,
    children_query: Query<&Children, With<RenameBar>>,
) {
    let Ok(mut node) = bar.single_mut() else {
        return;
    };
    node.display = Display::Flex;
    if let Ok(children) = children_query.single() {
        for child in children.iter() {
            if let Ok(mut text) = texts.get_mut(child) {
                text.0 = format!("{}{}\u{258f}", prompt.subject.label(), prompt.text);
            }
        }
    }
}

fn handle_rename_input(
    mut commands: Commands,
    mut prompt: ResMut<RenamePrompt>,
    mut events: MessageReader<KeyboardInput>,
    mut armed: Local<bool>,
    mut client: Option<Single<&mut TmuxClient>>,
) {
    // NOTE: the bound key that opened the prompt (`,` / `$`) is still in the
    // shared KeyboardInput buffer; this reader has its own cursor, so skip the
    // open frame — drain past the opening key — or it is appended to the name.
    if !*armed {
        events.clear();
        *armed = true;
        return;
    }
    for ev in events.read() {
        if ev.state != ButtonState::Pressed {
            continue;
        }
        match prompt.apply_key(&ev.logical_key) {
            RenameStep::Continue => {}
            RenameStep::Submit => {
                let cmd = prompt.submit_command();
                if let Some(client) = client.as_deref_mut()
                    && let Err(e) = client.send_raw(&cmd)
                {
                    tracing::warn!(?e, "rename submit failed");
                }
                commands.remove_resource::<RenamePrompt>();
                *armed = false;
                break;
            }
            RenameStep::Cancel => {
                commands.remove_resource::<RenamePrompt>();
                *armed = false;
                break;
            }
        }
    }
}

/// Extracts the inner command template from a `command-prompt` invocation: the
/// content of the trailing `{ ... }` brace group, else the last quoted/bare
/// token after the flags. Returns `None` if no template is present.
fn command_prompt_inner(tokens: &[String]) -> Option<String> {
    if let Some(open) = tokens.iter().position(|t| t == "{") {
        let close = tokens.iter().rposition(|t| t == "}")?;
        if close <= open + 1 {
            return None;
        }
        return Some(tokens[open + 1..close].join(" "));
    }
    let last = tokens.last()?;
    if last.starts_with('-') {
        return None;
    }
    Some(last.clone())
}

/// True when a token references `%2`..`%9` (a multi-prompt response slot).
fn has_high_arity_placeholder(token: &str) -> bool {
    let bytes = token.as_bytes();
    for i in 0..bytes.len() {
        if bytes[i] == b'%'
            && let Some(&next) = bytes.get(i + 1)
            && (b'2'..=b'9').contains(&next)
        {
            return true;
        }
    }
    false
}

/// Classifies an inner template's argv as a canonical rename, or `None`.
/// Accepts `<verb> [--] <placeholder>` where placeholder is exactly `%%` or
/// `%1` (after stripping surrounding quotes the tokenizer already removed).
fn rename_kind_of(inner: &[String]) -> Option<RenameKind> {
    let mut it = inner.iter();
    let kind = match it.next().map(String::as_str)? {
        "rename-window" | "renamew" => RenameKind::Window,
        "rename-session" | "rename" => RenameKind::Session,
        _ => return None,
    };
    let mut next = it.next().map(String::as_str)?;
    if next == "--" {
        next = it.next().map(String::as_str)?;
    }
    if next != "%%" && next != "%1" {
        return None;
    }
    if it.next().is_some() {
        return None;
    }
    Some(kind)
}

/// Splits a tmux command line into tokens, honoring single/double quotes (quotes
/// stripped, whitespace inside preserved) and treating `{` / `}` as standalone
/// tokens. Empty quoted tokens (`''`) yield an empty token.
fn tokenize(line: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut started = false;
    let mut in_single = false;
    let mut in_double = false;
    for c in line.chars() {
        match c {
            '\'' if !in_double => {
                in_single = !in_single;
                started = true;
            }
            '"' if !in_single => {
                in_double = !in_double;
                started = true;
            }
            '{' | '}' if !in_single && !in_double => {
                if started {
                    tokens.push(std::mem::take(&mut cur));
                    started = false;
                }
                tokens.push(c.to_string());
            }
            c if c.is_whitespace() && !in_single && !in_double => {
                if started {
                    tokens.push(std::mem::take(&mut cur));
                    started = false;
                }
            }
            c => {
                cur.push(c);
                started = true;
            }
        }
    }
    if started {
        tokens.push(cur);
    }
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::input::ButtonState;
    use bevy::input::keyboard::KeyCode;

    fn win_subject() -> RenameSubject {
        RenameSubject::Window {
            id: WindowId(2),
            current_name: "nvim".to_string(),
        }
    }

    fn char_key(s: &str) -> Key {
        Key::Character(s.into())
    }

    #[test]
    fn new_prefills_text_from_current_name() {
        let p = RenamePrompt::new(win_subject());
        assert_eq!(p.text, "nvim");
    }

    #[test]
    fn escape_cancels() {
        let mut p = RenamePrompt::new(win_subject());
        assert_eq!(p.apply_key(&Key::Escape), RenameStep::Cancel);
    }

    #[test]
    fn enter_submits() {
        let mut p = RenamePrompt::new(win_subject());
        assert_eq!(p.apply_key(&Key::Enter), RenameStep::Submit);
    }

    #[test]
    fn char_appends_and_continues() {
        let mut p = RenamePrompt::new(win_subject());
        p.text.clear();
        assert_eq!(p.apply_key(&char_key("a")), RenameStep::Continue);
        assert_eq!(p.apply_key(&char_key("b")), RenameStep::Continue);
        assert_eq!(p.text, "ab");
    }

    #[test]
    fn backspace_pops_last_char() {
        let mut p = RenamePrompt::new(win_subject());
        assert_eq!(p.apply_key(&Key::Backspace), RenameStep::Continue);
        assert_eq!(p.text, "nvi");
    }

    #[test]
    fn submit_command_uses_window_builder() {
        let p = RenamePrompt {
            subject: RenameSubject::Window {
                id: WindowId(2),
                current_name: "old".to_string(),
            },
            text: "new name".to_string(),
        };
        assert_eq!(p.submit_command(), "rename-window -t @2 -- 'new name'");
    }

    #[test]
    fn submit_command_uses_session_builder() {
        let p = RenamePrompt {
            subject: RenameSubject::Session {
                id: SessionId(1),
                current_name: "old".to_string(),
            },
            text: "proj".to_string(),
        };
        assert_eq!(p.submit_command(), "rename-session -t $1 -- proj");
    }

    #[test]
    fn label_matches_subject() {
        assert_eq!(
            RenameSubject::Window {
                id: WindowId(0),
                current_name: String::new(),
            }
            .label(),
            "Rename window: "
        );
        assert_eq!(
            RenameSubject::Session {
                id: SessionId(0),
                current_name: String::new(),
            }
            .label(),
            "Rename session: "
        );
    }

    #[test]
    fn recognizes_default_window_binding() {
        assert_eq!(
            RenameKind::parse(r##"command-prompt -I "#W" { rename-window -- "%%" }"##),
            Some(RenameKind::Window)
        );
    }

    #[test]
    fn recognizes_default_session_binding() {
        assert_eq!(
            RenameKind::parse(r##"command-prompt -I "#S" { rename-session -- "%%" }"##),
            Some(RenameKind::Session)
        );
    }

    #[test]
    fn recognizes_aliases() {
        assert_eq!(
            RenameKind::parse(r##"command-prompt -I "#W" { renamew -- "%%" }"##),
            Some(RenameKind::Window)
        );
        assert_eq!(
            RenameKind::parse(r##"command-prompt -I "#S" { rename -- "%%" }"##),
            Some(RenameKind::Session)
        );
    }

    #[test]
    fn recognizes_quoted_template_form() {
        assert_eq!(
            RenameKind::parse(r##"command-prompt -I "#W" "rename-window -- '%%'""##),
            Some(RenameKind::Window)
        );
    }

    #[test]
    fn recognizes_percent_one_placeholder() {
        assert_eq!(
            RenameKind::parse(r#"command-prompt { rename-window -- "%1" }"#),
            Some(RenameKind::Window)
        );
    }

    #[test]
    fn rejects_multi_input_by_arity() {
        assert_eq!(
            RenameKind::parse(r#"command-prompt -p a,b { rename-window -- "%1-%2" }"#),
            None
        );
    }

    #[test]
    fn accepts_literal_flag_with_comma_prompt() {
        assert_eq!(
            RenameKind::parse(r#"command-prompt -l -p "a,b" { rename-window -- "%%" }"#),
            Some(RenameKind::Window)
        );
    }

    #[test]
    fn accepts_comma_inside_single_quoted_prompt() {
        assert_eq!(
            RenameKind::parse(r#"command-prompt -p "new (was a, b)" { rename-window -- "%%" }"#),
            Some(RenameKind::Window)
        );
    }

    #[test]
    fn rejects_decorated_template() {
        assert_eq!(
            RenameKind::parse(r#"command-prompt { rename-window -- "[%%]" }"#),
            None
        );
    }

    #[test]
    fn rejects_copy_mode_search_prompt() {
        assert_eq!(
            RenameKind::parse(
                r#"command-prompt -T search -p "(search down)" { send-keys -X search-forward "%%%" }"#
            ),
            None
        );
    }

    #[test]
    fn rejects_confirm_before() {
        assert_eq!(RenameKind::parse("confirm-before kill-window"), None);
    }

    #[test]
    fn rejects_run_shell_containing_rename_window() {
        assert_eq!(RenameKind::parse(r#"run-shell "rename-window x""#), None);
    }

    fn key_event(logical: Key, code: KeyCode) -> KeyboardInput {
        KeyboardInput {
            key_code: code,
            logical_key: logical,
            state: ButtonState::Pressed,
            text: None,
            repeat: false,
            window: Entity::PLACEHOLDER,
        }
    }

    fn armed_skip_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<KeyboardInput>()
            .add_systems(
                Update,
                handle_rename_input.run_if(resource_exists::<RenamePrompt>),
            );
        app
    }

    #[test]
    fn open_frame_skips_the_opening_key() {
        let mut app = armed_skip_app();
        app.world_mut()
            .insert_resource(RenamePrompt::new(win_subject()));
        // The opening `,` is still in the shared buffer when the prompt opens.
        app.world_mut()
            .resource_mut::<Messages<KeyboardInput>>()
            .write(key_event(Key::Character(",".into()), KeyCode::Comma));
        app.update();

        let p = app.world().resource::<RenamePrompt>();
        assert_eq!(
            p.text, "nvim",
            "the opening key must not leak into the prefilled text"
        );
    }

    #[test]
    fn escape_after_open_frame_removes_resource() {
        let mut app = armed_skip_app();
        app.world_mut()
            .insert_resource(RenamePrompt::new(win_subject()));
        // Open frame drains the opening key.
        app.world_mut()
            .resource_mut::<Messages<KeyboardInput>>()
            .write(key_event(Key::Character(",".into()), KeyCode::Comma));
        app.update();
        assert!(app.world().get_resource::<RenamePrompt>().is_some());

        // Next frame: Escape cancels → resource removed.
        app.world_mut()
            .resource_mut::<Messages<KeyboardInput>>()
            .write(key_event(Key::Escape, KeyCode::Escape));
        app.update();
        assert!(
            app.world().get_resource::<RenamePrompt>().is_none(),
            "Escape must close the prompt"
        );
    }
}
