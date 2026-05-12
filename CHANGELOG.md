# Changelog

## Unreleased

### Breaking changes

- HTTP routes are now hierarchical under
  `/windows/{window_id}/panes/{pane_id}/...`. The legacy flat routes
  (`POST /panes`, `POST /panes/{pane_id}/split`,
  `POST /panes/{src}/split-with`, `DELETE /panes/{pane_id}`,
  `POST /activities`, `GET /activities/{aid}/terminal/ws`,
  `GET /activities/{aid}/handlers/ws`,
  `GET /activities/{aid}/iframe/{*path}`) are removed.
- SDK's `createActivity`, `createPane`, and `splitPane` (and their argument
  types) are removed. Use `pane.split({ activity: {...} })` to atomically
  allocate a pane plus its activity, or `pane.addActivity(...)` to add an
  activity to an existing pane.
- iframe SDK no longer infers `activityId` from `window.location.pathname`.
  `inferActivityId()` is removed. `createClient()` no longer accepts an
  `activityId` option; the daemon injects `window.__OZMUX__` into iframe HTML
  responses and the SDK reads from there.

### Added

- `<script>window.__OZMUX__ = {...}</script>` is injected into iframe HTML so
  the SDK can discover its `(sessionId, windowId, paneId, activityId)`
  position without parsing the URL.
- Per-Window concurrency: independent Windows no longer serialize on a single
  root lock. Each Window has its own `Arc<Mutex<Window>>`.
- Client-generated UUIDs for `pane_id` and `activity_id`
  (`crypto.randomUUID()` in the SDK).
- Atomic `pane.split({ orientation, side, activity })` API.

### Removed

- `LimboStore` from the daemon's `AppState` — the limbo activity/pane concept
  is gone. Activities and panes are always created inside a Window.
- `MultiplexerService` facade.
- `inferActivityId()` from the iframe SDK.
- All flat `/panes/...` and `/activities/...` routes (the `/sessions`,
  `/windows`, and `/health` roots remain flat).
- `sdk/typescript/src/server/deprecated.ts`.

### Bug fixes

- Restore extension-ownership registration on the hierarchical pane / activity
  creation paths. When the limbo handlers were removed, the only call sites
  for `ExtensionRegistry::record_activity_owner` /
  `record_pane_owner` went with them, so the iframe's handlers WebSocket
  upgrade always landed on a 404. The SDK now sends `extension_name` inside
  the Extension activity payload, and the daemon registers it as part of the
  split / add-activity transaction.
- The split handler no longer spawns a PTY for Extension activities. Previously
  every successful extension split also forked a shell that nothing ever read.
