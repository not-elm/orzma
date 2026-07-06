# Orzma Webview Protocol Doc Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Write `docs/orzma_webview_protocol.md` — a complete, language-agnostic specification of the Orzma Webview protocol (control socket + OSC 5379 + `window.orzma`) — and link to it from `README.md`.

**Architecture:** A single new English Markdown file under `docs/`, built up section-by-section in connection order (control socket → OSC 5379 → `window.orzma`), each section appended in its own task and verified against the Rust/TS source. `README.md` gets one link line. No code changes.

**Tech Stack:** Markdown only. Verification uses `grep`/file reads against the orzma Rust crates and the `@orzma/web` SDK. No build step; `docs/**` is not linted (Biome scans `sdk/**` only).

## Global Constraints

- **Language:** English only (matches `README.md`, `docs/configs.md`). Verbatim copy from the spec.
- **Audience/scope:** integrator-facing wire contract. Document observable behavior only; no ECS/CEF/Bevy internals.
- **Doc path:** `docs/orzma_webview_protocol.md` (lives in `docs/`, so SDK links are `../sdk/...`).
- **Markdown conventions:** `#` title, `##` sections, `###` subsections; inline backticks for messages/fields/escape-seqs/env-vars; fenced blocks tagged `json` (NDJSON), `ts` (`window.orzma`), and a plain/`text` block for the OSC byte grammar and the diagram.
- **Source of truth:** every factual claim must match the files listed in the spec's "Source-of-truth reference" section (`docs/specs/2026-06-28-orzma-webview-protocol-doc-design.md`). Do not re-derive or guess values; if a value in this plan disagrees with the source at implementation time, trust the source and flag it in the task's review.
- **Three non-obvious contract points MUST appear, each stated explicitly:** (a) register reply has no `op`, every push has `op`; (b) `window.orzma.emit` reaches the program as `op:"event"`, a program `emit` reaches pages as `op:"emit"`; (c) only top-level `Uint8Array` round-trips, nested bytes are lost.
- **TDD adaptation:** there is no executable test for prose. Each task's "verify" step is a factual cross-check against the named source file(s) with the exact expected values given. Treat a mismatch as a failing test: fix the doc (or flag the source discrepancy) before committing.
- **Commits:** one commit per task. This work is on the `osc` branch; commit locally, do not push.

---

### Task 1: Scaffold the doc — title, Overview (§1), Architecture diagram (§2)

**Files:**
- Create: `docs/orzma_webview_protocol.md`
- Reference (read-only): `docs/specs/2026-06-28-orzma-webview-protocol-doc-design.md`, `README.md:52-59`, `CLAUDE.md:14-38`

**Interfaces:**
- Consumes: nothing (first task).
- Produces: the file with `# Orzma Webview Protocol`, a `## Overview` section, and a `## Architecture at a glance` section ending before `## The control socket`. Later tasks append sections in order.

- [ ] **Step 1: Write the title, Overview, and Architecture diagram**

Write `docs/orzma_webview_protocol.md` with exactly this content:

```markdown
# Orzma Webview Protocol

> orzma is in early development; this wire format is documented as it is today
> and may change between releases. The [SDKs](#sdks) track these changes for
> you — prefer them unless you are implementing your own client.

The Orzma Webview protocol lets a local program running inside an orzma pane
render webview content inline in the terminal and exchange messages with the
page. It spans three surfaces:

1. **The control socket** — a local Unix-socket connection over which a program
   registers content, manages it, and routes the page back-channel.
2. **OSC 5379** — terminal escape sequences that mount and unmount registered
   content at a cell rectangle.
3. **The `window.orzma` bridge** — an in-page JavaScript API the webview uses to
   call, subscribe to, and emit events to the registering program.

Three actors participate: the **registering program** (running in a pane), the
**orzma host**, and the **webview page**. A registration is a *Tier 1* (dynamic,
runtime-registered) webview — the only kind this protocol describes.

End to end: a program connects to the control socket, registers content and
receives an opaque **handle**, writes an `OSC 5379;mount;<handle>;…` sequence to
display it, and then talks to the page through the `window.orzma` bridge routed
over the same control socket. Unmounting (or disconnecting) tears it down.

## Architecture at a glance

\`\`\`text
 registering program              orzma host                  webview page
 (inside an orzma pane)
        │  reads $ORZMA_SOCK / $ORZMA_TOKEN from its env
        │  hello{token} ───────────────►│
        │  register{kind,…} ───────────►│
        │◄─────────────── {ok,handle} ──│
        │  OSC 5379;mount;handle;r;c ──►│  mount orzma://handle/ ───►│ load page
        │                               │◄──── window.orzma.call ────│
        │◄──── {op:call,reqId,method} ──│                           │
        │  {op:reply,reqId,value} ─────►│──── resolve Promise ─────►│
        │  {op:emit,event,payload} ────►│──── window.orzma.on ──────►│
        │◄──── {op:event,…} ◄ window.orzma.emit ─────────────────────│
        │  OSC 5379;unmount;handle ────►│  despawn webview ─────────►│
\`\`\`

The control socket carries every horizontal arrow between the program and the
host; OSC 5379 carries the mount/unmount; the page bridge carries the
`window.orzma` arrows on the right.
```

(Note: the `\`\`\`text` fences above are escaped for this plan — write them as real triple-backtick fences in the file.)

- [ ] **Step 2: Verify the framing against source**

Read `CLAUDE.md:14-38` and confirm the doc's three surfaces and "register over control socket → mint handle → OSC mount → `window.orzma` back-channel" flow match. Confirm "Tier 1" matches `crates/orzma_webview/src/control_plane.rs:1-4` (listener "accepts authenticated dynamic webview registrations (Tier 1)").
Run: `grep -n "Tier 1" crates/orzma_webview/src/control_plane.rs`
Expected: matches confirming "Tier 1" = dynamic registration.

- [ ] **Step 3: Commit**

```bash
git add docs/orzma_webview_protocol.md
git commit -m "docs(webview): scaffold protocol spec with overview and diagram"
```

---

### Task 2: Control socket — connection, discovery, handshake, framing (§3 part 1)

**Files:**
- Modify: `docs/orzma_webview_protocol.md` (append `## The control socket` and its connection subsections)
- Reference: `crates/orzma_webview/src/control_plane/listener.rs`, `crates/orzma_webview/src/control_plane.rs:322-360`

**Interfaces:**
- Consumes: the file from Task 1.
- Produces: the `## The control socket` heading plus subsections "Transport", "Discovery", "Peer authentication", "Handshake", "Reply vs. push", "Register ordering". The next task appends the message tables under the same `## The control socket` section.

- [ ] **Step 1: Append the connection subsections**

Append to `docs/orzma_webview_protocol.md`:

```markdown
## The control socket

### Transport

The control socket is a local Unix **stream** socket speaking **NDJSON**:
exactly one JSON object per line, terminated by `\n` (a trailing `\r` is
tolerated). Each line travels in one direction. The connection is long-lived —
it stays open for as long as the program wants its registrations to live.

### Discovery

orzma injects two environment variables into every pane's PTY. A program reads
them from its own environment:

- `$ORZMA_SOCK` — the absolute path to the control socket. Connect to this path
  verbatim; do not reconstruct it.
- `$ORZMA_TOKEN` — the per-pane handshake token. Treat it as opaque (it is
  currently of the form `orzma:<bits>`, but do not parse it).

If either variable is absent, the program is not running inside an orzma pane
and cannot use the protocol.

### Peer authentication

The host checks that the connecting peer's user id equals orzma's own user id
and silently drops the connection otherwise. Only processes running as the same
user can connect.

### Handshake

The **first** line a program sends MUST be a `hello` carrying `$ORZMA_TOKEN`:

\`\`\`json
{"op":"hello","token":"orzma:4294967306"}
\`\`\`

The token binds the connection to the pane it was issued for. If the first line
is not a valid `hello`, or the token does not resolve, the host closes the
connection without a reply. A second `hello` on an already-handshaked
connection is ignored.

### Reply vs. push

After the handshake, two kinds of line arrive **from** the host on the same
connection, and a client must tell them apart:

- A **register reply** is the only host line with **no `op` field** — it is
  either `{"ok":true,"handle":"…"}` or `{"ok":false,"error":"…"}`.
- Every **host-initiated push** (`call`, `event`, `compositing`) carries an
  `op` field.

So: a line with an `op` is a push; a line without one is the reply to your most
recent `register`. This is the one framing rule a from-scratch client must get
right.

### Register ordering

Registrations are processed one at a time per connection, and each
`register`'s reply arrives in request order. `register` has no request id —
correlation is positional. (The back-channel `call`/`reply` pair below uses an
explicit `reqId` instead.)
```

- [ ] **Step 2: Verify against source**

Confirm each fact:
- `$ORZMA_SOCK` / `$ORZMA_TOKEN` names: `grep -n "ORZMA_SOCK\|ORZMA_TOKEN" crates/orzma_webview/src/control_plane.rs` (expect `surface_env` at ~338-346 returning both).
- Peer-UID check: `grep -n "peer_uid\|getpeereid\|SO_PEERCRED" crates/orzma_webview/src/control_plane/listener.rs` (expect the check dropping non-matching UIDs).
- hello-first + drop on bad token: `read_hello` in `listener.rs:244-255` (returns `None` → connection returns early).
- Reply has no `op`: `ServerMsg` in `crates/orzma_webview/src/control_plane/protocol.rs:146-163` is `#[serde(untagged)]` with only `ok`/`handle`/`error`. Push has `op`: `PushMsg` is `#[serde(tag = "op")]`.
- Register ordering: `handle_client_msg` Register arm in `listener.rs:268-288` recv's the reply then sends it before reading the next line.

Expected: all confirmed.

- [ ] **Step 3: Commit**

```bash
git add docs/orzma_webview_protocol.md
git commit -m "docs(webview): document control-socket connection and framing"
```

---

### Task 3: Control socket — messages, register kinds, errors, handle, example (§3 part 2)

**Files:**
- Modify: `docs/orzma_webview_protocol.md` (append message tables and worked example under `## The control socket`)
- Reference: `crates/orzma_webview/src/control_plane/protocol.rs`, `crates/orzma_webview/src/control_plane.rs:664-759` (`build_view`, `validate_url_source`, error codes), `crates/orzma_webview/src/webview/render.rs:151-272` (host→program `call`/`event`/`urlChanged`)

**Interfaces:**
- Consumes: the `## The control socket` section from Task 2.
- Produces: subsections "Program → host messages", "Register kinds", "Forward keys", "Host → program messages", "Register reply & error codes", "Handle semantics", "Example exchange".

- [ ] **Step 1: Append the message reference and example**

Append to `docs/orzma_webview_protocol.md`:

```markdown
### Program → host messages

Every program line carries an `op`:

| `op` | Fields | Meaning |
| --- | --- | --- |
| `hello` | `token` | Handshake; first line only. |
| `register` | `kind` + per-kind fields | Register content, mint a handle. |
| `unregister` | `handle` | Release a handle owned by this connection; despawns its mounted views. |
| `reply` | `reqId`, `ok`, `value?`, `error?` | Answer a host `call` (use the `call`'s `reqId`). |
| `emit` | `handle`, `event`, `payload` | Push an event to the handle's pages (delivered to `window.orzma.on`). |
| `focus` | `handle` (string or `null`), `instance` (string or `null`) | Set app-owned focus to a mounted view, or `null` to blur. |
| `navigate` | `handle`, `action` | Navigate a mounted view in place. |

`navigate.action` is one of the strings `"back"`, `"forward"`, `"reload"`, or
the object `{"to":"<http(s) url>"}` (`to` is valid only on a `url` view).

### Register kinds

`register` carries a `kind` discriminator and its fields:

| `kind` | Required | Optional (default) | Served at |
| --- | --- | --- | --- |
| `dir` | `root` (absolute dir path), `entry` (safe relative path, e.g. `index.html`) | `interactive` (`true`), `forward_keys` (`[]`), `preload` (`[]`) | `orzma://<handle>/` |
| `inline` | `html` (full document, ≤ 4 MiB) | `interactive` (`true`), `forward_keys` (`[]`), `preload` (`[]`) | `orzma://<handle>/index.html` |
| `url` | `url` (`http`/`https` only) | `interactive` (`true`), `bridge` (`false`), `forward_keys` (`[]`), `preload` (`[]`) | the remote URL directly (no `orzma://` origin) |

- `interactive` — whether the mounted view accepts pointer/keyboard input.
- `bridge` (`url` only) — opt into the `window.orzma` back-channel. `dir` and
  `inline` are always bridged; a `url` view is bridged only with `bridge:true`.
- `preload` — an array of JavaScript source strings injected before the page's
  own scripts (after the host bridge). Honored only for bridged views.

### Forward keys

`forward_keys` lists key chords the host passes through to the pane's PTY
instead of letting the focused webview consume them. Each chord is:

\`\`\`json
{"mods":["alt"],"key":"h"}
\`\`\`

`mods` is any subset of `"alt"`, `"ctrl"`, `"shift"`, `"meta"`. `key` is one of:
a lowercase letter `a`–`z`, a digit `0`–`9`, `tab`, `backtab`, `f1`–`f12`,
`esc`, `" "` (space), `up`, `down`, `pageup`, `pagedown`. Unrecognized chords
are silently ignored.

### Host → program messages

Every host push carries an `op`:

| `op` | Fields | Meaning / response |
| --- | --- | --- |
| `call` | `handle`, `reqId`, `method`, `params` | A page `window.orzma.call(method, params)`. Respond with a `reply` carrying the same `reqId`. |
| `event` | `handle`, `event`, `payload` | A page `window.orzma.emit(event, payload)`. Fire-and-forget; no response. |
| `compositing` | `handle`, `active` (bool) | The view first composited (`true`) or was unmounted after compositing (`false`). |

Two directional details that are easy to get wrong:

- **`emit` vs. `event`.** A page's `window.orzma.emit(name, …)` arrives at the
  program as `op:"event"`. A program's own `emit` message (`op:"emit"`) is
  delivered to pages' `window.orzma.on(name, …)`. Same idea ("named event"), two
  `op` values depending on direction.
- **`urlChanged`.** For a `url` view, the host reports top-level address changes
  as an `op:"call"` with `method:"urlChanged"` and `params:{"url":"<new>"}`.
  Despite the `call` shape it is fire-and-forget — any `reply` is discarded. Use
  it to track page-driven navigation.

### Register reply & error codes

A successful `register` replies `{"ok":true,"handle":"<handle>"}`. A rejected one
replies `{"ok":false,"error":"<code>"}`:

| `error` | Cause |
| --- | --- |
| `invalid_root` | `dir.root` is not an absolute path to an existing directory. |
| `unsafe_entry` | `dir.entry` is empty, absolute, or contains `..`/`.`. |
| `html_too_large` | `inline.html` exceeds 4 MiB. |
| `invalid_url` | `url.url` does not parse or has no host. |
| `unsupported_scheme` | `url.url` is not `http`/`https`. |
| `internal` | The host failed to process the request. |

### Handle semantics

A handle is opaque, unique per registration, lowercase, and matches
`^[a-z0-9._-]{1,128}$` (a subset of the OSC `view_id` charset, so a handle is
always a valid `mount` argument). Treat it as a token: do not parse it. Each
handle owns one isolated `orzma://<handle>/` origin.

### Example exchange

Program-to-host lines are marked `C→S`, host-to-program lines `S→C`:

\`\`\`json
C→S {"op":"hello","token":"orzma:4294967306"}
C→S {"op":"register","kind":"inline","html":"<!doctype html><body>hi</body>"}
S→C {"ok":true,"handle":"nf2k7q9w3x1m5a8b0c4d6e7f"}
S→C {"op":"call","handle":"nf2k7q9w3x1m5a8b0c4d6e7f","reqId":"0","method":"save","params":{"text":"hi"}}
C→S {"op":"reply","reqId":"0","ok":true,"value":{"saved":true}}
C→S {"op":"emit","handle":"nf2k7q9w3x1m5a8b0c4d6e7f","event":"tick","payload":{"n":1}}
\`\`\`
```

- [ ] **Step 2: Verify against source**

- Client message set + fields: `crates/orzma_webview/src/control_plane/protocol.rs:10-67` (`ClientMsg` variants hello/register/unregister/reply/emit/focus/navigate; `focus` fields `handle`/`instance` both `Option`).
- `NavAction`: `protocol.rs:70-81` (back/forward/reload/`to`).
- Register kinds + defaults: `protocol.rs:92-143` (`Dir` requires `root`+`entry`; `interactive` default true via `default_true`; `Url` has `bridge` default false). 4 MiB cap: `crates/orzma_webview/src/control_plane.rs:847` (`MAX_INLINE_HTML = 4 * 1024 * 1024`).
- forward_keys key names: `crates/orzma_webview/src/control_plane.rs:765-823` (`normalize_chord` accepted strings; mods alt/ctrl/shift/meta).
- Host→program `call`/`event`/`urlChanged` shapes: `crates/orzma_webview/src/webview/render.rs:178-187, 228-231, 263-270`.
- `compositing` shape: `protocol.rs:185-196` (`PushMsg::Compositing { handle, active }`).
- Error codes: `crates/orzma_webview/src/control_plane.rs:679,682,700,720` (`invalid_root`, `unsafe_entry`, `html_too_large`) and `validate_url_source` `750-758` (`invalid_url`, `unsupported_scheme`); `internal` at `listener.rs:283`.
- Handle charset/lowercase: `control_plane.rs:298-311` + the test `minted_ids_match_the_osc_view_id_charset` (`control_plane.rs:891-909`).

Expected: every value in the doc matches.

- [ ] **Step 3: Commit**

```bash
git add docs/orzma_webview_protocol.md
git commit -m "docs(webview): document control-socket messages, kinds, and errors"
```

---

### Task 4: OSC 5379 — mount / unmount (§4)

**Files:**
- Modify: `docs/orzma_webview_protocol.md` (append `## OSC 5379 — mount / unmount`)
- Reference: `crates/orzma_tty_engine/src/osc/webview.rs`

**Interfaces:**
- Consumes: the handle concept from Task 3 (the `view_id` is the registered handle).
- Produces: the `## OSC 5379 — mount / unmount` section.

- [ ] **Step 1: Append the OSC section**

Append to `docs/orzma_webview_protocol.md`:

```markdown
## OSC 5379 — mount / unmount

Once a handle is registered, the program mounts it by writing an OSC 5379 escape
sequence to its terminal. The sequence is framed `ESC ] 5379 ; <params> ST`,
where `ST` (string terminator) is `ESC \` or `BEL`. In raw bytes:

\`\`\`text
mount:    \x1b]5379;mount;<view_id>;<rows>;<cols>\x1b\
unmount:  \x1b]5379;unmount;<view_id>\x1b\
\`\`\`

### mount

\`\`\`text
OSC 5379 ; mount ; <view_id> ; <rows> ; <cols> [ ; <instance_id> ] ST
\`\`\`

- `view_id` — the handle from `register`; charset `^[A-Za-z0-9._-]{1,128}$`.
- `rows` — decimal `1`–`200`. `cols` — decimal `1`–`400`. Digits only, no sign.
- `instance_id` — optional, same charset as `view_id`. It lets one handle mount
  several independent placements. A trailing empty field (`mount;<id>;3;20;`) is
  malformed.

The view occupies a `rows`×`cols` rectangle of terminal cells, inline at the
cursor.

### unmount

\`\`\`text
OSC 5379 ; unmount [ ; <view_id> [ ; <instance_id> ] ] ST
\`\`\`

- No `view_id` → unmount all of this program's inline views on the terminal.
- `view_id` only → unmount that handle's default instance.
- `view_id` + `instance_id` → unmount that specific placement.

An `instance_id` is addressable only alongside a `view_id`.

### Ownership and malformed sequences

A `mount;<handle>` takes effect only in the pane whose `$ORZMA_TOKEN` registered
that handle — a program mounts its own handles in its own pane. Any malformed
sequence (bad charset, out-of-range dimensions, empty fields) is silently
dropped; the host reports no error.

### Example

Mount handle `nf2k7q9w3x1m5a8b0c4d6e7f` as a 24×80 view, then unmount it:

\`\`\`text
\x1b]5379;mount;nf2k7q9w3x1m5a8b0c4d6e7f;24;80\x1b\
\x1b]5379;unmount;nf2k7q9w3x1m5a8b0c4d6e7f\x1b\
\`\`\`
```

(Write `\x1b\` as backslash-x-1-b-backslash literally; it denotes `ESC` then the `\` of `ST`. The closing `\` in `ESC \` is shown as a single `\`.)

- [ ] **Step 2: Verify against source**

- Limits and charset: `crates/orzma_tty_engine/src/osc/webview.rs:13-15` (`MAX_VIEW_ID=128`, `MAX_ROWS=200`, `MAX_COLS=400`), `valid_view_id:39-50` (charset `[A-Za-z0-9._-]`, len 1–128), `parse_dim:52-59` (digits only, `1..=max`).
- mount params order (view_id, rows, cols, instance_id): `osc_dispatch:66-90`.
- unmount addressing (absent = all, instance only with view_id): `osc_dispatch:91-120` and the NOTE at `92-107`.
- Trailing-empty malformed: tests `mount_trailing_empty_instance_id_dropped:314-325`, `unmount_absent_param_is_all_but_empty_param_is_malformed:340-366`.

Expected: all confirmed.

- [ ] **Step 3: Commit**

```bash
git add docs/orzma_webview_protocol.md
git commit -m "docs(webview): document OSC 5379 mount/unmount grammar"
```

---

### Task 5: The `orzma://` origin & asset serving (§5)

**Files:**
- Modify: `docs/orzma_webview_protocol.md` (append `## The orzma:// origin`)
- Reference: `crates/webview_host/src/orzma_scheme.rs`, `crates/webview_host/src/asset.rs`

**Interfaces:**
- Consumes: handle/origin concept from Task 3, the `dir`/`inline`/`url` kinds from Task 3.
- Produces: the `## The orzma:// origin` section.

- [ ] **Step 1: Append the origin section**

Append to `docs/orzma_webview_protocol.md`:

```markdown
## The `orzma://` origin

`dir` and `inline` registrations are served from a per-handle origin
`orzma://<handle>/`. A request for an empty path resolves to `index.html`. The
scheme is standard, secure, CORS-enabled, fetch-enabled, and display-isolated,
so normal `fetch`, ES modules, and same-origin requests work within the
handle's origin. Each handle is its own isolated origin.

- **`dir`** — files are served from the registered `root`. Path traversal
  (`..`, absolute paths, percent-encoded escapes) is rejected; each file is
  capped at 64 MiB; the content type is inferred from the file extension.
- **`inline`** — the single registered document is served only at `index.html`;
  any subresource request returns 404. Use `dir` for multi-file content.
- **`url`** — the remote `http(s)` page is loaded directly and has **no**
  `orzma://` origin.
```

- [ ] **Step 2: Verify against source**

- Scheme options + name: `crates/webview_host/src/orzma_scheme.rs` `custom_orzma_scheme` (`STANDARD|SECURE|CORS_ENABLED|FETCH_ENABLED|DISPLAY_ISOLATED`, name `orzma`).
- Default `index.html` + handle/path parse: `parse_orzma_url` in `orzma_scheme.rs`.
- Inline only at `index.html`; traversal rejection; 64 MiB cap; MIME by extension: `crates/webview_host/src/asset.rs` (`MAX_ASSET_LEN`, `is_safe_rel_path`, extension→MIME map).

Expected: confirmed.

- [ ] **Step 3: Commit**

```bash
git add docs/orzma_webview_protocol.md
git commit -m "docs(webview): document the orzma:// origin and asset serving"
```

---

### Task 6: The `window.orzma` bridge (§6)

**Files:**
- Modify: `docs/orzma_webview_protocol.md` (append `## The window.orzma bridge`)
- Reference: `sdk/orzma-web/src/orzma.ts`, `crates/orzma_webview/src/webview/render/orzma_bridge.js`

**Interfaces:**
- Consumes: bridged-view rule from Task 3 (`dir`/`inline` always; `url` with `bridge:true`).
- Produces: the `## The window.orzma bridge` section, including the binary round-trip rule and a TS example.

- [ ] **Step 1: Append the bridge section**

Append to `docs/orzma_webview_protocol.md`:

```markdown
## The `window.orzma` bridge

Bridged webviews expose a frozen `window.orzma` object to page scripts.
`dir` and `inline` views are always bridged; a `url` view is bridged only when
registered with `bridge:true`. A page should feature-detect before using it.

### API

| Method | Returns | Meaning |
| --- | --- | --- |
| `call(method, params?)` | `Promise` | Invoke a program method; resolves with the program's `reply` value, rejects with `Error(error)`. |
| `on(event, handler)` | `void` | Subscribe to a program `emit`. |
| `off(event, handler)` | `void` | Remove a handler by reference. |
| `emit(event, payload?)` | `void` | Send a one-way event to the program (arrives as `op:"event"`). |

A `call` has **no client-side timeout** — if the program never replies, the
Promise stays pending. The host injects a rejection when it cannot route the
call: `no_owner` (the view has no registering connection), `owner_unavailable`
(the connection's writer is gone), or `owner_disconnected` (the program
disconnected with the call in flight).

### Binary round-trip

A **top-level** `Uint8Array` round-trips through the bridge — it is tagged
`{"__u8":"<base64>"}` on the wire and decoded back to a `Uint8Array`. This
applies to a `call`'s `params`, a resolved `value`, and an event `payload`.
Bytes **nested** inside an object or array are **not** tagged and are silently
lost. Pass binary as the top-level value, not as a field.

### Example

Using the [`@orzma/web`](../sdk/orzma-web) client:

\`\`\`ts
import { orzma, isOrzmaAvailable } from "@orzma/web";

if (isOrzmaAvailable()) {
  // Request / response — annotate the reply type.
  const res = await orzma.call<{ saved: boolean }>("save", { text: "hi" });

  // Subscribe to a program event — annotate the payload.
  orzma.on<{ n: number }>("tick", (payload) => console.log(payload.n));

  // One-way event to the program.
  orzma.emit("ready", { ok: true });
}
\`\`\`
```

- [ ] **Step 2: Verify against source**

- API methods/signatures + nested-bytes caveat: `sdk/orzma-web/src/orzma.ts:4-35` (`OrzmaApi.call/on/off/emit`), `isOrzmaAvailable`/`orzma` exports at `58-76`, unavailable message at `52`.
- `{__u8}` top-level-only base64 tagging: `crates/orzma_webview/src/webview/render/orzma_bridge.js:1-9` (header NOTE), `encodeParam:17-24`, `decodeValue:25-33`.
- `window.orzma` frozen: `orzma_bridge.js:82`.
- Rejection codes `no_owner`/`owner_unavailable`: `crates/orzma_webview/src/webview/render.rs:173-185`; `owner_disconnected`: `crates/orzma_webview/src/control_plane.rs:505-508`.
- Availability rule (`is_bridged`): `crates/orzma_webview/src/control_plane.rs:65-74`.

Expected: confirmed.

- [ ] **Step 3: Commit**

```bash
git add docs/orzma_webview_protocol.md
git commit -m "docs(webview): document the window.orzma bridge and byte round-trip"
```

---

### Task 7: Lifecycle, security, and SDKs (§7–9)

**Files:**
- Modify: `docs/orzma_webview_protocol.md` (append `## Lifecycle & teardown`, `## Security model`, `## SDKs`)
- Reference: `crates/orzma_webview/src/control_plane.rs:483-509`, `crates/orzma_webview/src/control_plane.rs:201-223`, `README.md:45-50`

**Interfaces:**
- Consumes: handle, connection, `reqId` concepts from earlier tasks.
- Produces: the final three sections, including the `<a name>`/heading `## SDKs` that the Overview's `[SDKs](#sdks)` link in Task 1 targets.

- [ ] **Step 1: Append the closing sections**

Append to `docs/orzma_webview_protocol.md`:

```markdown
## Lifecycle & teardown

- `unregister{handle}` releases a handle and despawns its mounted views.
- Closing the control connection purges all of that program's handles, despawns
  their views, and rejects every in-flight `call` with `owner_disconnected`.
- The `compositing` push reports a view's first paint (`active:true`) and its
  teardown after compositing (`active:false`).

## Security model

- **Same user only.** The host rejects any control connection whose peer user id
  differs from orzma's.
- **Scoped to one pane.** A connection's token binds it to the pane that issued
  `$ORZMA_TOKEN`; a program may only mount, focus, navigate, and emit to handles
  it registered.
- **Unguessable, isolated handles.** Handles are 128-bit random values, each its
  own `orzma://` origin.
- **Authorized replies.** Back-channel `reqId`s are a shared, monotonic counter
  and therefore guessable, so the host authorizes a `reply` by its originating
  connection: a program replaying another connection's `reqId` can neither
  settle nor drop that call.

## SDKs

Prefer a ready-made client over implementing the wire protocol directly:

- [`ratatui_orzma`](../sdk/ratatui_orzma) — Rust SDK for the program side (a
  ratatui widget plus a back-channel RPC handler).
- [`@orzma/web`](../sdk/orzma-web) — TypeScript client for the page-side
  `window.orzma` bridge.
```

- [ ] **Step 2: Verify against source**

- unregister/disconnect teardown + `owner_disconnected` rejection: `crates/orzma_webview/src/control_plane.rs:483-509`.
- reqId ownership guarantee: `take_for_connection` doc + invariant at `control_plane.rs:201-223`.
- SDK names/paths match README: `grep -n "ratatui_orzma\|@orzma/web" README.md` (expect `README.md:47,49`).
- Confirm the in-doc anchor: the Overview link `[SDKs](#sdks)` resolves to this `## SDKs` heading (GitHub lowercases/​hyphenates → `#sdks`).

Expected: confirmed.

- [ ] **Step 3: Commit**

```bash
git add docs/orzma_webview_protocol.md
git commit -m "docs(webview): document lifecycle, security model, and SDKs"
```

---

### Task 8: Link the doc from README.md

**Files:**
- Modify: `README.md:52-59` (the `## Orzma Webview Protocol` section)

**Interfaces:**
- Consumes: the finished `docs/orzma_webview_protocol.md`.
- Produces: a link from the README to the new doc.

- [ ] **Step 1: Append the link line**

In `README.md`, find the `## Orzma Webview Protocol` paragraph that ends:

```
... through the `window.orzma` bridge. Use one of the SDKs above for a
ready-made client.
```

Immediately after that paragraph (before `## Configuration`), add a blank line
then:

```markdown
See [docs/orzma_webview_protocol.md](docs/orzma_webview_protocol.md) for the full
protocol specification.
```

Leave the existing paragraph unchanged.

- [ ] **Step 2: Verify the link target**

Run: `test -f docs/orzma_webview_protocol.md && grep -n "docs/orzma_webview_protocol.md" README.md`
Expected: the file exists and README contains the link.

- [ ] **Step 3: Commit**

```bash
git add README.md
git commit -m "docs: link to the Orzma Webview protocol specification"
```

---

### Task 9: Final verification & consistency pass

**Files:**
- Read-only review of `docs/orzma_webview_protocol.md` and the spec.

**Interfaces:**
- Consumes: the complete doc and README link.
- Produces: a verified, internally consistent document (fixes applied inline if needed).

- [ ] **Step 1: Acceptance-criteria checklist**

Confirm each item from the spec's "Acceptance criteria":
- The doc exists, is English, and has sections in this order: Overview, Architecture at a glance, The control socket, OSC 5379 — mount / unmount, The `orzma://` origin, The `window.orzma` bridge, Lifecycle & teardown, Security model, SDKs.
- Each control-socket message (both directions), the OSC grammar, the `orzma://`
  rules, and the `window.orzma` API are documented to field/byte level.
- The doc contains: one architecture diagram, one NDJSON example, one literal
  OSC byte example, one `window.orzma` TS snippet.
- The three non-obvious contract points are present: search the file for
  "no `op`"/reply-vs-push, the `emit`/`event` asymmetry, and the top-level-only
  `Uint8Array` rule.

Run: `grep -nc '^## ' docs/orzma_webview_protocol.md` (expect 9 section headings).

- [ ] **Step 2: Internal consistency**

- Handle charset stated identically wherever it appears (`^[a-z0-9._-]{1,128}$`
  for minted handles; `^[A-Za-z0-9._-]{1,128}$` for the OSC `view_id` field —
  these differ intentionally: minted handles are lowercase, the field accepts
  upper too. Confirm the doc says exactly that and does not contradict itself).
- `rows` 1–200 / `cols` 1–400 consistent between the OSC section and any
  mention elsewhere.
- 4 MiB inline cap and 64 MiB asset cap each stated once, not swapped.
- Every relative link resolves: `../sdk/orzma-web`, `../sdk/ratatui_orzma`, and
  the in-page `#sdks` anchor.

Run: `ls sdk/orzma-web sdk/ratatui_orzma >/dev/null && echo OK`
Expected: `OK`.

- [ ] **Step 3: Fix and commit any corrections**

If Steps 1–2 surfaced issues, fix them inline, then:

```bash
git add docs/orzma_webview_protocol.md README.md
git commit -m "docs(webview): final consistency pass on the protocol spec"
```

If no issues were found, skip the commit (nothing to commit) and note the doc is verified.

---

## Self-Review (completed by plan author)

**Spec coverage:** Every spec section maps to a task — §1–2 → Task 1; §3 →
Tasks 2–3; §4 → Task 4; §5 → Task 5; §6 → Task 6; §7–9 → Task 7; README change
→ Task 8; acceptance criteria → Task 9. No gaps.

**Placeholder scan:** No "TBD"/"TODO"/"handle edge cases"/"similar to Task N".
Each content step embeds the full Markdown to write; each verify step lists
exact files, line ranges, and expected values.

**Type/term consistency:** `view_id`/handle, `reqId`, `op`, the `emit`/`event`
asymmetry, and the cap values (4 MiB / 64 MiB, rows 1–200 / cols 1–400) are used
identically across tasks. The intentional charset difference (lowercase minted
handle vs. mixed-case OSC field) is called out in Task 9 so it is not "fixed"
into a false contradiction.
```
