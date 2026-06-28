# Design: `docs/ozma_webview_protocol.md` — Ozma Webview Protocol specification

## Goal

Write a new public document, `docs/ozma_webview_protocol.md`, that fully
specifies the Ozma Webview protocol so a developer can implement a client in
**any language** without reading the Rust source or relying on an SDK. Link to
it from `README.md`.

## Audience & scope

- **Reader:** integrators building an SDK or application against the protocol
  (not ozmux contributors).
- **Depth:** a complete, language-agnostic **wire specification** — exact OSC
  byte grammar, the control-socket NDJSON messages field-by-field, the
  `ozma://` origin rules, and the page-side `window.ozma` API.
- **Out of scope:** ozmux's internal ECS/CEF/Bevy mechanics (anchors, overlay
  slots, render systems, focus systems). Describe only **observable contract**
  behavior. Where an internal detail leaks into observable behavior (e.g.
  handles are lowercase because Chromium lowercases URL hosts), state the
  observable rule, not the implementation.
- **Language:** English (matches `README.md` and `docs/configs.md`, the
  existing English-only docs).

## Conventions to follow

- Markdown: `#` title, `##` sections, `###` subsections (mirrors
  `docs/configs.md`).
- Inline code in backticks for message names, fields, escape sequences, env
  vars (`$OZMA_SOCK`, `OSC 5379;mount;<handle>;<rows>;<cols>`, `window.ozma`).
- Relative links to SDKs: `[@ozma/web](../sdk/ozma-web)`,
  `[ratatui-ozma](../sdk/ratatui-ozma)` (the doc lives in `docs/`, so SDK paths
  are `../sdk/...`).
- Fenced code blocks: `json` for NDJSON lines, `ts` for `window.ozma` snippets,
  a literal block for the OSC byte grammar.
- Stay descriptive, not imperative, in prose; this is a reference, not a
  tutorial. A short end-to-end walkthrough is allowed up front.
- Note the early-development status: the wire format is documented "as it is
  today" and may change (the README already carries a breaking-changes
  caution). Frame the raw protocol as the contract the SDKs implement.

## Document structure (chosen approach: layered by surface, in connection order)

The doc follows the three wire surfaces in the order an integrator uses them,
with a single end-to-end diagram up front. Each surface section is
self-contained: concept → grammar/message tables → a concrete worked example.

### 1. Overview

- What the protocol is: a local program running in an ozmux pane registers
  webview content, mounts it inline in the terminal grid, and exchanges
  messages with the page.
- The three surfaces: (1) the **control socket** (register/manage + back-channel
  routing), (2) **OSC 5379** (mount/unmount), (3) the **`window.ozma`** page
  bridge.
- The actors: **registering program** ↔ **ozmux host** ↔ **webview page**.
- "Tier 1" = dynamically-registered webviews (the only kind this protocol
  covers). Define the term once; don't over-explain other tiers.
- One-paragraph end-to-end summary.

### 2. Architecture at a glance

- One ASCII sequence/data-flow diagram showing: connect+hello → register → OSC
  mount → page loads `ozma://<handle>/` → `window.ozma.call` → host `call` →
  program `reply` → `window.ozma` Promise resolves → OSC unmount / disconnect.
- Label which surface carries each arrow.

### 3. The control socket

- **Transport:** Unix stream socket; **NDJSON** — exactly one JSON object per
  line, `\n`-terminated (`\r\n` tolerated). One direction per line.
- **Discovery:** the program reads two env vars injected into its PTY:
  - `$OZMA_SOCK` — absolute path to the control socket. Clients MUST use this
    value; do not reconstruct the path.
  - `$OZMA_TOKEN` — the per-pane handshake token (opaque string, currently of
    the form `ozma:<bits>`; treat as opaque).
- **Peer authentication:** the listener checks the connecting peer's UID equals
  the ozmux process UID and silently drops the connection otherwise. (Observable
  effect: only same-user processes can connect.)
- **Handshake:** the **first** line MUST be `hello` carrying `$OZMA_TOKEN`. If
  the token does not resolve (or the first line is not a valid `hello`), the
  host drops the connection with no reply. Bind the connection to the pane that
  owns the token.
- **Reply vs. push discrimination (critical):** a `register` reply is the only
  line **without** an `op` field (`{"ok":…}`); every host-initiated push line
  **has** an `op` field. A client distinguishes a register reply from an
  interleaved push by the presence of `op`. Document this explicitly — it is the
  one non-obvious framing rule a from-scratch client must get right.
- **Request/reply ordering:** `register` is processed one-at-a-time per
  connection; its reply arrives in request order. There is no request id on
  `register`. (Back-channel `call`/`reply` use an explicit `reqId`.)

#### Program → host messages (field tables + one example each)

| `op` | Fields | Meaning |
| --- | --- | --- |
| `hello` | `token` | Handshake (first line only). |
| `register` | `kind` + per-kind fields (below) | Register content, mint a handle. |
| `unregister` | `handle` | Release a handle owned by this connection. |
| `reply` | `reqId`, `ok`, `value?`, `error?` | Answer a host-initiated `call`. |
| `emit` | `handle`, `event`, `payload` | Push an event to the handle's pages (`window.ozma.on`). |
| `focus` | `handle` (string or `null`), `instance` (string or `null`) | Set/clear app-owned focus on the owning surface. |
| `navigate` | `handle`, `action` | In-place nav of a mounted view. |

- `navigate.action` is one of the strings `"back"`, `"forward"`, `"reload"`, or
  the object `{"to": "<http(s) url>"}`. `to` is only valid on a `url` view.
- **Register kinds:**
  - `dir` — `root` (absolute path; must exist and be a directory), `entry`
    (safe relative path, e.g. `index.html`; no `..`/absolute), `interactive`
    (default `true`), `forward_keys` (default `[]`), `preload` (default `[]`).
    Served at `ozma://<handle>/`.
  - `inline` — `html` (full document, ≤ **4 MiB**), `interactive`,
    `forward_keys`, `preload`. Served at `ozma://<handle>/index.html`.
  - `url` — `url` (`http`/`https` only), `interactive`, `bridge` (default
    `false` — opt-in to `window.ozma`), `forward_keys`, `preload`. Loaded
    directly; no `ozma://` origin.
- **`forward_keys` chord shape:** `{"mods": [...], "key": "<name>"}`. `mods` ⊆
  `{"alt","ctrl","shift","meta"}`. `key` ∈ lowercase `a`–`z`, `0`–`9`, `tab`,
  `backtab`, `f1`–`f12`, `esc`, `" "` (space), `up`, `down`, `pageup`,
  `pagedown`. Unrecognized chords are silently ignored. Document what
  `forward_keys` is *for*: chords listed here are passed through to the pane's
  PTY instead of being consumed by the focused webview.
- **`preload`:** array of JS source strings injected before the page's own
  scripts (after the host bridge). Only honored for bridged views.

#### Host → program messages (all carry `op`)

| `op` | Fields | Meaning / expected response |
| --- | --- | --- |
| `call` | `handle`, `reqId`, `method`, `params` | A page `window.ozma.call(method, params)`. Respond with `reply` carrying the same `reqId`. |
| `event` | `handle`, `event`, `payload` | A page `window.ozma.emit(event, payload)`. Fire-and-forget; no response. |
| `compositing` | `handle`, `active` (bool) | The view first composited (`true`) or was unmounted after compositing (`false`). |

- **Direction asymmetry to call out explicitly:** a page's
  `window.ozma.emit(event, …)` arrives at the program as `op: "event"`
  (inbound), whereas a program's `emit` message (`op: "emit"`) is delivered to
  pages' `window.ozma.on(event, …)`. Same concept ("named event"), two `op`
  values by direction. A from-scratch client must not confuse them.
- **`urlChanged`:** for a `url` view, top-level address changes are delivered as
  an `op: "call"` with `method: "urlChanged"`, `params: {"url": "<new>"}`. It is
  fire-and-forget despite being shaped as a `call` (any `reply` is dropped).
  Document this as the way a program tracks page-driven navigation.

#### Register reply

- Success: `{"ok": true, "handle": "<opaque handle>"}`.
- Failure: `{"ok": false, "error": "<code>"}`. Codes: `invalid_root`,
  `unsafe_entry`, `html_too_large`, `invalid_url`, `unsupported_scheme`,
  `internal`. Tabulate each with its cause.

#### Handle semantics

- Opaque, unique per registration, lowercase, from the charset
  `^[a-z0-9._-]{1,128}$` (a subset of the OSC `view_id` charset, so a handle is
  always a valid `mount` argument). Treat as an opaque token; do not parse.
- One handle = one `ozma://<handle>/` origin (isolated from other handles).

#### Worked example

A literal NDJSON exchange: `hello` → `register inline` → `{"ok":true,...}` →
(page calls) host `call` → program `reply`.

### 4. OSC 5379 — mount / unmount

- **Framing:** `ESC ] 5379 ; <params> ST`, where `ST` is `ESC \` (string
  terminator) or `BEL`. Show the literal bytes (`\x1b]5379;...\x1b\\`).
- **mount grammar:**
  `OSC 5379 ; mount ; <view_id> ; <rows> ; <cols> [ ; <instance_id> ] ST`
  - `view_id` — the handle from `register`; `^[A-Za-z0-9._-]{1,128}$`.
  - `rows` — decimal `1`–`200`. `cols` — decimal `1`–`400`. Digits only (no
    sign).
  - `instance_id` — optional, same charset as `view_id`; lets one handle mount
    multiple independent placements. A trailing empty field
    (`mount;<h>;3;20;`) is malformed.
- **unmount grammar:**
  `OSC 5379 ; unmount [ ; <view_id> [ ; <instance_id> ] ] ST`
  - No `view_id` → unmount all of this program's inline views on the terminal.
  - `view_id` only → unmount that handle's default instance.
  - `view_id` + `instance_id` → unmount that specific placement.
  - An `instance_id` is only addressable alongside a `view_id`.
- **Malformed sequences are silently dropped** (no error reported to the
  program). List the validation rules so a client emits well-formed sequences.
- **Ownership:** a `mount;<handle>` only takes effect in the pane whose
  `$OZMA_TOKEN` registered that handle (a program mounts its own handles in its
  own pane).
- **Geometry:** `rows`/`cols` are terminal cells; the view occupies that cell
  rectangle inline.
- **Worked example:** a literal mount byte sequence and its unmount.

### 5. The `ozma://` origin & asset serving

- URL shape: `ozma://<handle>/<path>`; empty path → `index.html`.
- One origin per handle; standard, secure, CORS-enabled, fetch-enabled,
  display-isolated scheme (so normal `fetch`, ES modules, etc. work, scoped to
  the handle's origin).
- `dir`: files served from the registered root; path traversal (`..`, absolute,
  encoded) rejected; per-file size cap **64 MiB**; MIME inferred by extension.
- `inline`: the single document is served only at `index.html`; subresource
  requests 404 (use `dir` for multi-file content).
- `url`: remote `http(s)` loaded directly; **no** `ozma://` origin.

### 6. The `window.ozma` bridge (page side)

- **Availability:** present only on **bridged** views — `dir`/`inline` always;
  `url` only when registered with `bridge: true`. A page should feature-detect
  (`isOzmaAvailable()` / `typeof window.ozma`).
- **API** (mirror `OzmaApi` from `sdk/ozma-web`):
  - `call(method, params?) → Promise` — resolves with the program's `reply`
    value, rejects with `Error(error)`. Host-injected rejection codes:
    `no_owner`, `owner_unavailable`, `owner_disconnected`. **No client-side
    timeout** — a call with no reply stays pending (the host sends an error
    reply on owner disconnect).
  - `on(event, handler)` / `off(event, handler)` — subscribe/unsubscribe to a
    program `emit`.
  - `emit(event, payload?)` — one-way to the program (arrives as `op:"event"`).
- **Binary round-trip rule:** a **top-level** `Uint8Array` round-trips (tagged
  `{"__u8": "<base64>"}` on the wire) in `call` params, a resolved `value`, and
  an event `payload`. Bytes **nested** inside an object/array do **not**
  round-trip and are silently lost. State this prominently — it is a common
  foot-gun.
- **TS worked example** using `@ozma/web`.

### 7. Lifecycle & teardown

- Explicit `unregister` releases a handle and despawns its mounted views.
- Closing the control connection purges all the program's handles, despawns
  their views, and rejects in-flight `call`s with `owner_disconnected`.
- `compositing` push signals first paint (`active:true`) and teardown
  (`active:false`).

### 8. Security model

- Same-user only (peer-UID check).
- The pane token scopes a connection to one surface; a program can only mount,
  focus, navigate, and emit to **its own** handles.
- Handles are unguessable (128-bit random) and origin-isolated.
- Back-channel `reqId`s are a shared monotonic counter (guessable), so the host
  authorizes `reply` by **originating connection**: a foreign program replaying
  another connection's `reqId` cannot settle or drop that call. Present this as
  a contract guarantee, not as ECS internals.

### 9. SDKs

- `[ratatui-ozma](../sdk/ratatui-ozma)` — Rust client (the program side).
- `[@ozma/web](../sdk/ozma-web)` — TypeScript client for `window.ozma` (the page
  side).
- Recommend the SDKs as the supported path; the wire spec above is for those
  implementing their own client.

## README.md change

Keep the existing `## Ozma Webview Protocol` section's short prose (lines
54–59) and append one link line, matching the `## Configuration` section's
pattern (`See [docs/configs.md](docs/configs.md) for ...`):

> See [docs/ozma_webview_protocol.md](docs/ozma_webview_protocol.md) for the
> full protocol specification.

No other README change.

## Acceptance criteria

- `docs/ozma_webview_protocol.md` exists, is English, and follows the section
  structure above.
- Every control-socket message (both directions), the OSC grammar, the
  `ozma://` rules, and the `window.ozma` API are documented to field/byte
  level, each consistent with the verified facts below.
- The doc contains: one architecture diagram, one NDJSON worked example, one
  literal OSC byte example, one `window.ozma` TS snippet.
- The three non-obvious contract points are stated explicitly: (a) `op`-absent
  reply vs. `op`-present push; (b) `emit`/`event` direction asymmetry; (c)
  top-level-only `Uint8Array` round-trip.
- `README.md` links to the new doc from the existing protocol section.
- No claim in the doc contradicts the source references below.

## Source-of-truth reference (verified against the code 2026-06-28)

The doc author should not re-derive these; they were read directly from source.

- OSC grammar & limits — `crates/ozma_tty_engine/src/osc/webview.rs`
  (`MAX_VIEW_ID=128`, `MAX_ROWS=200`, `MAX_COLS=400`; `valid_view_id` charset
  `[A-Za-z0-9._-]`; mount/unmount param rules and malformed-drop behavior).
- Control-socket NDJSON messages — `crates/ozma_webview/src/control_plane/protocol.rs`
  (`ClientMsg`: hello/register/unregister/reply/emit/focus/navigate;
  `RegisterKind`: dir/inline/url with defaults; `ServerMsg` untagged
  ok/err — no `op`; `PushMsg::Compositing`; `NavAction` back/forward/reload/to;
  `HostKeyChord`).
- Handle minting, env injection, register validation/error codes, ownership —
  `crates/ozma_webview/src/control_plane.rs` (`mint_id` lowercase base32;
  `surface_env` injects `OZMA_SOCK`/`OZMA_TOKEN`; `build_view` error codes
  `invalid_root`/`unsafe_entry`/`html_too_large` and `validate_url_source`
  `invalid_url`/`unsupported_scheme`; `MAX_INLINE_HTML=4 MiB`; `OzmaSource::is_bridged`;
  `normalize_chord` key names; `take_for_connection` ownership invariant).
- Listener transport, peer-UID auth, hello-first, NDJSON framing —
  `crates/ozma_webview/src/control_plane/listener.rs` (`peer_uid` via
  `getpeereid`/`SO_PEERCRED`; `read_hello`; register reply relayed in order;
  server-push lines carry `op`).
- Host→program `call`/`event`/`urlChanged` wire shapes —
  `crates/ozma_webview/src/webview/render.rs` (`{"op":"call",handle,reqId,method,params}`,
  `{"op":"event",handle,event,payload}`, `urlChanged` fire-and-forget).
- Page bridge & byte tagging — `crates/ozma_webview/src/webview/render/ozma_bridge.js`
  (`ozma.call`→`{kind:"ozma.call",reqId,method,params}`;
  `ozma.emit`→`{kind:"ozma.emit",event,payload}`; replies on `ozma` channel,
  events on `ozma.event`; `{__u8: base64}` top-level only; `window.ozma` frozen).
- `ozma://` scheme & asset serving — `crates/webview_host/src/ozma_scheme.rs`
  and `crates/webview_host/src/asset.rs` (scheme options STANDARD|SECURE|CORS|
  FETCH|DISPLAY_ISOLATED; default `index.html`; inline only at `index.html`;
  traversal rejection; 64 MiB cap; MIME by extension).
- Page-side API surface — `sdk/ozma-web/src/ozma.ts` (`OzmaApi`,
  `isOzmaAvailable`, `ozma`; `call`/`on`/`off`/`emit` signatures and the
  nested-bytes caveat in JSDoc).
</content>
</invoke>
