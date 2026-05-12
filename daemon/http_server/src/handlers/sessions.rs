use ozmux_multiplexer::SessionId;

pub mod create;
pub mod delete;
pub mod get;
pub mod list;
pub mod rename;

pub(crate) fn session_view(
    id: &SessionId,
    session: &ozmux_multiplexer::Session,
) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "name": session.name,
        "windows": session.linked_windows,
        "active_window": session.active_window,
    })
}
