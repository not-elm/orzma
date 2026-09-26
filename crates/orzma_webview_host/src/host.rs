//! The webview host: every client's state and every decision about it,
//! driven by control-socket events, VT placement signals, and pane
//! lifecycle changes.

use crate::boundary::{ForwardChord, HandleId, MountId, Navigation, WebviewCommand, WebviewEvent};
use crate::control_socket::{ConnectionId, ControlEvent, ControlSocket};
use crate::error::{Refusal, RegisterError, WebviewHostResult};
use crate::host::calls::InFlightCalls;
use crate::host::connections::Connections;
use crate::host::focus::{FocusRoute, FocusState, FocusTransition};
use crate::host::mint::mint_instance_id;
use crate::host::mounts::{MountChange, MountState, Mounts};
use crate::host::registry::{Registration, Registry};
use crate::host::tokens::Tokens;
use crate::host::validation::validate_url;
use crate::protocol::{NavAction, PushMsg, ServerMsg};
use crossbeam_channel::{Receiver, Sender, TryRecvError};
use orzma_vt::prelude::{
    GridColumn, InstanceId, MAX_COLS, MAX_ROWS, PlacementSize, ScreenLine, VtSignal,
};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::fmt::Debug;
use std::hash::Hash;

mod calls;
mod connections;
mod focus;
pub(crate) mod mint;
mod mounts;
mod registry;
mod tokens;
mod validation;

pub use validation::ValidatedRegistration;

/// The bounds a pane key needs: the host copies, compares, and hashes it.
pub trait PaneKey: Copy + Eq + Hash + Debug {}

impl<T: Copy + Eq + Hash + Debug> PaneKey for T {}

/// What one call into the host produced: events for the GUI and requests
/// for the multiplexer, each in the order they arose.
#[must_use]
#[derive(Debug, PartialEq)]
pub struct HostOutput<P> {
    events: Vec<WebviewEvent<P>>,
    requests: Vec<MuxRequest<P>>,
}

impl<P> Default for HostOutput<P> {
    fn default() -> Self {
        Self {
            events: Vec::new(),
            requests: Vec::new(),
        }
    }
}

impl<P> HostOutput<P> {
    /// The events for the GUI, in order.
    pub fn events(&self) -> &[WebviewEvent<P>] {
        &self.events
    }

    /// The requests for the multiplexer, in order.
    pub fn requests(&self) -> &[MuxRequest<P>] {
        &self.requests
    }

    /// Splits the output into its events and its requests.
    pub fn into_parts(self) -> (Vec<WebviewEvent<P>>, Vec<MuxRequest<P>>) {
        (self.events, self.requests)
    }

    fn push_event(&mut self, event: WebviewEvent<P>) {
        self.events.push(event);
    }

    fn push_request(&mut self, request: MuxRequest<P>) {
        self.requests.push(request);
    }
}

/// An operation the host asks the multiplexer to apply to a pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MuxRequest<P> {
    /// Mount `instance` at the visible cell `(row, column)` of `pane`.
    MountPlacement {
        /// The pane to mount in.
        pane: P,
        /// The placement to mount.
        instance: InstanceId,
        /// The visible row the rect's top edge sits on.
        row: ScreenLine,
        /// The column the rect's left edge sits on.
        column: GridColumn,
        /// The rect's extent in cells.
        size: PlacementSize,
    },
    /// Drop the reservations `pane` holds for `instances`; ids the pane does
    /// not hold are ignored.
    RemovePlacements {
        /// The pane holding the reservations.
        pane: P,
        /// The placements to drop.
        instances: Vec<InstanceId>,
    },
    /// Make `pane` the active pane.
    SelectPane {
        /// The pane to activate.
        pane: P,
    },
}

/// A webview placement change a pane's VT reported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlacementSignal {
    /// The VT accepted a mount of `instance`.
    Mounted {
        /// The placement mounted.
        instance: InstanceId,
        /// The rect it reserved.
        size: PlacementSize,
    },
    /// The VT refused a mount at its placement cap.
    Rejected {
        /// The placement refused.
        instance: InstanceId,
    },
    /// The PTY unmounted one placement, or every placement when `None`.
    Unmounted {
        /// The placement unmounted.
        instance: Option<InstanceId>,
    },
    /// The VT dropped placements on its own: history trim, reset,
    /// alternate-screen teardown, or resize.
    Evicted {
        /// The placements dropped.
        instances: Vec<InstanceId>,
    },
}

impl TryFrom<VtSignal> for PlacementSignal {
    type Error = VtSignal;

    fn try_from(signal: VtSignal) -> Result<Self, VtSignal> {
        match signal {
            VtSignal::WebviewMount { instance, size } => Ok(Self::Mounted { instance, size }),
            VtSignal::WebviewMountRejected { instance } => Ok(Self::Rejected { instance }),
            VtSignal::WebviewUnmount { instance } => Ok(Self::Unmounted { instance }),
            VtSignal::WebviewEvicted { placements } => Ok(Self::Evicted {
                instances: placements,
            }),
            other => Err(other),
        }
    }
}

/// Every client's state, and every decision about it.
///
/// `P` identifies a pane. The host never touches a pane itself: each entry
/// point returns the events the GUI applies and the requests the
/// multiplexer carries out.
pub struct WebviewHost<P> {
    socket: Option<ControlSocket>,
    connections: Connections<P>,
    tokens: Tokens<P>,
    registry: Registry<P>,
    mounts: Mounts<P>,
    focus: FocusState<P>,
    active: Option<P>,
    calls: InFlightCalls,
    composited: HashMap<MountId, CompositeRoute>,
}

impl<P: PaneKey> WebviewHost<P> {
    /// A host without a control socket: it hands panes no environment and
    /// receives no control events.
    pub fn without_socket() -> Self {
        Self::new(None)
    }

    /// A host serving `socket`.
    pub fn with_socket(socket: ControlSocket) -> Self {
        Self::new(Some(socket))
    }

    /// The channel the listener sends control events on, or `None` without
    /// a socket.
    pub fn control_events(&self) -> Option<&Receiver<ControlEvent>> {
        self.socket.as_ref().map(ControlSocket::events)
    }

    /// The next queued control event, or `None` when none is queued or the
    /// host has no socket.
    ///
    /// Once every sender of the control channel is gone (the listener
    /// stopped), the host drops its socket: later panes start without
    /// `ORZMA_SOCK` / `ORZMA_TOKEN`, and [`control_events`](Self::control_events)
    /// returns `None` from then on.
    pub fn try_recv_control(&mut self) -> Option<ControlEvent> {
        let socket = self.socket.as_ref()?;
        match socket.events().try_recv() {
            Ok(event) => Some(event),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                tracing::error!(
                    "the control-socket listener stopped; webview clients can no longer connect"
                );
                self.socket = None;
                None
            }
        }
    }

    /// Marks `pane` live and returns the environment its shell starts with:
    /// `ORZMA_SOCK` and a freshly bound `ORZMA_TOKEN`, or nothing when the
    /// host has no socket.
    ///
    /// # Errors
    ///
    /// Returns [`WebviewHostError::Csprng`](crate::error::WebviewHostError::Csprng)
    /// when the token cannot be minted; the pane stays live without a token.
    pub fn bind_pane(&mut self, pane: P) -> WebviewHostResult<Vec<(String, String)>> {
        self.tokens.mark_live(pane);
        let Some(socket) = &self.socket else {
            return Ok(Vec::new());
        };
        let sock = socket.sock_path().to_string_lossy().into_owned();
        let token = self.tokens.issue(pane)?;
        Ok(vec![
            ("ORZMA_SOCK".to_string(), sock),
            ("ORZMA_TOKEN".to_string(), token),
        ])
    }

    /// Forgets `pane`: its token stops resolving, and the registrations it
    /// owns are released with their mounts and assets. Its VT is gone, so no
    /// reservation release is requested. Connections from the pane stay
    /// open: a later `register` from one is refused with `owner_gone`, and a
    /// request naming one of its released handles finds no registration.
    pub fn pane_closed(&mut self, pane: P) -> HostOutput<P> {
        self.tokens.forget(pane);
        let mut output = HostOutput::default();
        let released = self.registry.remove_by_pane(pane);
        self.release(&mut output, released, Reservations::Gone);
        let leftover = self.mounts.on_pane(pane);
        self.end_mounts(&mut output, &leftover);
        output
    }

    /// Applies one placement change `pane`'s VT reported.
    ///
    /// A mount of an instance that is unknown, or owned by another pane, is
    /// answered with a request to drop its reservation.
    pub fn placement_signal(&mut self, pane: P, signal: PlacementSignal) -> HostOutput<P> {
        let mut output = HostOutput::default();
        match signal {
            PlacementSignal::Mounted { instance, size } => {
                self.placement_mounted(&mut output, pane, instance, size);
            }
            PlacementSignal::Rejected { instance } => {
                tracing::debug!(%instance, ?pane, "the VT refused a mount at its placement cap");
            }
            PlacementSignal::Unmounted {
                instance: Some(instance),
            } => self.end_pane_mounts(&mut output, pane, &[instance]),
            PlacementSignal::Unmounted { instance: None } => {
                let all = self.mounts.on_pane(pane);
                self.end_mounts(&mut output, &all);
            }
            PlacementSignal::Evicted { instances } => {
                self.end_pane_mounts(&mut output, pane, &instances);
            }
        }
        output
    }

    /// Records that `active` is now the active pane, and releases webview
    /// focus held by a mount in any other pane.
    pub fn active_pane_changed(&mut self, active: Option<P>) -> HostOutput<P> {
        self.active = active;
        let mut output = HostOutput::default();
        let elsewhere = self
            .focus
            .current()
            .is_some_and(|route| Some(route.pane()) != active);
        if elsewhere && self.clear_focus() {
            output.push_event(WebviewEvent::FocusChanged { focused: None });
        }
        output
    }

    /// Applies one command the GUI sent.
    ///
    /// A `Focus` is always answered with a `FocusChanged` carrying the
    /// resulting focus. A `PageCall` is answered with a `PageReply` at once
    /// when it cannot be forwarded: `no_owner` when its mount has ended or
    /// has no bridge, and `owner_unavailable` when its program cannot be
    /// written to.
    ///
    /// # Errors
    ///
    /// Returns [`WebviewHostError::Refused`](crate::error::WebviewHostError::Refused)
    /// for a `Composited`, `PageEmit`, or `UrlChanged` naming an ended
    /// mount, a `PageEmit` with an empty name or from a page without the
    /// bridge, and a `UrlChanged` of a page that is not a bridged remote
    /// page; a push that cannot reach its program fails the same way.
    pub fn command(&mut self, command: WebviewCommand) -> WebviewHostResult<HostOutput<P>> {
        match command {
            WebviewCommand::Focus { mount } => Ok(self.gui_focus(mount)),
            WebviewCommand::Composited { mount } => self.composited(mount),
            WebviewCommand::PageCall {
                mount,
                page_req,
                method,
                params,
            } => Ok(self.page_call(mount, page_req, method, params)),
            WebviewCommand::PageEmit {
                mount,
                event,
                payload,
            } => self.page_emit(mount, event, payload),
            WebviewCommand::UrlChanged { mount, url } => self.url_changed(mount, url),
        }
    }

    /// Applies one control event.
    ///
    /// `hello`, `register`, and `new_instance` are answered on their reply
    /// channels, with a wire error code when they fail, and never return an
    /// error.
    ///
    /// # Errors
    ///
    /// Returns [`WebviewHostError::Refused`](crate::error::WebviewHostError::Refused)
    /// when a request without a reply names a handle or instance its
    /// connection does not own, an instance not spelled as 32 hex digits, a
    /// mount size out of range, or a `focus` of a placement that is not
    /// mounted or takes no input, an `emit`, `navigate`, or
    /// `set_forward_keys` of a handle or instance the connection does not
    /// own, an `emit` of a handle without the bridge, and a `navigate` to an
    /// invalid URL or of an unmounted placement or of a page that is not a
    /// remote page. Nothing changes then.
    pub fn control(&mut self, event: ControlEvent) -> WebviewHostResult<HostOutput<P>> {
        match event {
            ControlEvent::Hello {
                connection,
                token,
                writer,
                reply,
            } => {
                self.hello(connection, &token, writer, &reply);
                Ok(HostOutput::default())
            }
            ControlEvent::Register {
                connection,
                registration,
                reply,
            } => Ok(self.register(connection, registration, &reply)),
            ControlEvent::NewInstance {
                connection,
                handle,
                reply,
            } => {
                self.new_instance(connection, &handle, &reply);
                Ok(HostOutput::default())
            }
            ControlEvent::Unregister { connection, handle } => self.unregister(connection, &handle),
            ControlEvent::Disconnect { connection } => Ok(self.disconnect(connection)),
            ControlEvent::Mount {
                connection,
                instance,
                row,
                col,
                rows,
                cols,
            } => self.socket_mount(
                connection,
                &instance,
                row,
                col,
                PlacementSize { rows, cols },
            ),
            ControlEvent::Unmount {
                connection,
                instance,
            } => self.socket_unmount(connection, &instance),
            ControlEvent::Focus {
                connection,
                instance,
            } => self.socket_focus(connection, instance),
            ControlEvent::Reply {
                connection,
                req_id,
                ok,
                value,
                error,
            } => Ok(self.reply(connection, &req_id, ok, value, error)),
            ControlEvent::Emit {
                connection,
                handle,
                event,
                payload,
            } => self.emit(connection, &handle, event, payload),
            ControlEvent::Navigate {
                connection,
                instance,
                action,
            } => self.navigate(connection, &instance, action),
            ControlEvent::SetForwardKeys {
                connection,
                handle,
                keys,
            } => self.set_forward_keys(connection, &handle, keys),
        }
    }

    fn new(socket: Option<ControlSocket>) -> Self {
        Self {
            socket,
            connections: Connections::new(),
            tokens: Tokens::new(),
            registry: Registry::new(),
            mounts: Mounts::new(),
            focus: FocusState::new(),
            active: None,
            calls: InFlightCalls::new(),
            composited: HashMap::new(),
        }
    }

    fn hello(
        &mut self,
        connection: ConnectionId,
        token: &str,
        writer: Sender<String>,
        reply: &Sender<bool>,
    ) {
        let pane = self.tokens.resolve(token);
        if let Some(pane) = pane {
            self.connections.insert(connection, pane, writer);
        }
        let _ = reply.send(pane.is_some());
    }

    fn register(
        &mut self,
        connection: ConnectionId,
        registration: Result<ValidatedRegistration, RegisterError>,
        reply: &Sender<ServerMsg>,
    ) -> HostOutput<P> {
        match self.try_register(connection, registration) {
            Ok((handle, instance, output)) => {
                let _ = reply.send(ServerMsg::registered(handle, instance));
                output
            }
            Err(error) => {
                if !error.is_refusal() {
                    tracing::warn!(%error, ?connection, "a register failed in the host");
                }
                let _ = reply.send(ServerMsg::err(error.wire_code()));
                HostOutput::default()
            }
        }
    }

    fn try_register(
        &mut self,
        connection: ConnectionId,
        registration: Result<ValidatedRegistration, RegisterError>,
    ) -> WebviewHostResult<(HandleId, InstanceId, HostOutput<P>)> {
        let pane = self.live_pane_of(connection)?;
        let content = registration?;
        let handle = HandleId::mint()?;
        let instance = mint_instance_id()?;
        let asset = content.asset();
        self.registry.insert(
            handle.clone(),
            Registration::new(content, pane, connection, instance),
        )?;
        let mut output = HostOutput::default();
        if let Some(asset) = asset {
            output.push_event(WebviewEvent::AssetRegistered {
                handle: handle.clone(),
                asset,
            });
        }
        Ok((handle, instance, output))
    }

    fn new_instance(
        &mut self,
        connection: ConnectionId,
        handle: &HandleId,
        reply: &Sender<ServerMsg>,
    ) {
        let answer = match self.try_new_instance(connection, handle) {
            Ok(instance) => ServerMsg::instanced(instance),
            Err(error) => {
                if !error.is_refusal() {
                    tracing::warn!(%error, ?connection, %handle, "a new_instance failed in the host");
                }
                ServerMsg::err(error.wire_code())
            }
        };
        let _ = reply.send(answer);
    }

    fn try_new_instance(
        &mut self,
        connection: ConnectionId,
        handle: &HandleId,
    ) -> WebviewHostResult<InstanceId> {
        let registration = self.registry.get(handle).ok_or(Refusal::UnknownHandle)?;
        if !self.tokens.is_live(registration.owner_pane()) {
            return Err(Refusal::OwnerGone.into());
        }
        if registration.connection() != connection {
            return Err(Refusal::NotOwner.into());
        }
        let instance = mint_instance_id()?;
        self.registry.add_instance(handle, instance)?;
        Ok(instance)
    }

    fn unregister(
        &mut self,
        connection: ConnectionId,
        handle: &HandleId,
    ) -> WebviewHostResult<HostOutput<P>> {
        self.owned_registration(connection, handle)?;
        let mut output = HostOutput::default();
        let released: Vec<_> = self
            .registry
            .remove(handle)
            .map(|registration| (handle.clone(), registration))
            .into_iter()
            .collect();
        self.release(&mut output, released, Reservations::Held);
        Ok(output)
    }

    fn disconnect(&mut self, connection: ConnectionId) -> HostOutput<P> {
        self.connections.remove(connection);
        let mut output = HostOutput::default();
        for (mount, page_req) in self.calls.drain_connection(connection) {
            output.push_event(WebviewEvent::PageReply {
                mount,
                page_req,
                outcome: Err("owner_disconnected".into()),
            });
        }
        let released = self.registry.remove_by_connection(connection);
        self.release(&mut output, released, Reservations::Held);
        output
    }

    fn socket_mount(
        &mut self,
        connection: ConnectionId,
        instance: &str,
        row: u16,
        col: u16,
        size: PlacementSize,
    ) -> WebviewHostResult<HostOutput<P>> {
        let (instance, pane) = self.owned_instance(connection, instance)?;
        if size.rows == 0 || MAX_ROWS < size.rows || size.cols == 0 || MAX_COLS < size.cols {
            return Err(Refusal::SizeOutOfRange.into());
        }
        let mut output = HostOutput::default();
        output.push_request(MuxRequest::MountPlacement {
            pane,
            instance,
            row: ScreenLine(row),
            column: GridColumn(col),
            size,
        });
        Ok(output)
    }

    fn socket_unmount(
        &mut self,
        connection: ConnectionId,
        instance: &str,
    ) -> WebviewHostResult<HostOutput<P>> {
        let (instance, pane) = self.owned_instance(connection, instance)?;
        let mut output = HostOutput::default();
        self.end_mounts(&mut output, &[instance]);
        output.push_request(MuxRequest::RemovePlacements {
            pane,
            instances: vec![instance],
        });
        Ok(output)
    }

    fn gui_focus(&mut self, mount: Option<MountId>) -> HostOutput<P> {
        let mut output = HostOutput::default();
        match mount {
            Some(mount) => {
                if let Err(error) = self.focus_mount(&mut output, mount) {
                    tracing::debug!(?mount, %error, "a focus from the GUI was refused");
                }
            }
            None => {
                self.clear_focus();
            }
        }
        output.push_event(WebviewEvent::FocusChanged {
            focused: self.focus.focused_mount(),
        });
        output
    }

    fn socket_focus(
        &mut self,
        connection: ConnectionId,
        instance: Option<String>,
    ) -> WebviewHostResult<HostOutput<P>> {
        let mut output = HostOutput::default();
        let changed = match instance {
            Some(spelled) => {
                let (instance, _) = self.owned_instance(connection, &spelled)?;
                let mount = self
                    .mounts
                    .get(instance)
                    .map(MountState::mount)
                    .ok_or(Refusal::NotMounted)?;
                self.focus_mount(&mut output, mount)?
            }
            None => {
                let pane = self
                    .connections
                    .pane_of(connection)
                    .ok_or(Refusal::ConnectionClosed)?;
                let in_own_pane = self
                    .focus
                    .current()
                    .is_some_and(|route| route.pane() == pane);
                in_own_pane && self.clear_focus()
            }
        };
        if changed {
            output.push_event(WebviewEvent::FocusChanged {
                focused: self.focus.focused_mount(),
            });
        }
        Ok(output)
    }

    /// Moves focus to `mount`, pushing `false` to the mount that lost it and
    /// `true` to `mount`'s owner, and asking for `mount`'s pane to be
    /// selected when it is not the active one. Returns whether focus moved.
    fn focus_mount(
        &mut self,
        output: &mut HostOutput<P>,
        mount: MountId,
    ) -> WebviewHostResult<bool> {
        let route = self.focus_route(mount)?;
        let pane = route.pane();
        let FocusTransition::Moved { lost } = self.focus.set(route.clone()) else {
            return Ok(false);
        };
        if let Some(lost) = lost {
            self.push_focus(&lost, false);
        }
        self.push_focus(&route, true);
        if self.active != Some(pane) {
            output.push_request(MuxRequest::SelectPane { pane });
        }
        Ok(true)
    }

    /// The focus route of `mount`.
    fn focus_route(&self, mount: MountId) -> WebviewHostResult<FocusRoute<P>> {
        let (instance, state) = self.mounts.resolve(mount).ok_or(Refusal::StaleMount)?;
        let (handle, registration) = self
            .registry
            .resolve_instance(instance)
            .ok_or(Refusal::UnknownInstance)?;
        if !registration.content().interactive() {
            return Err(Refusal::NotInteractive.into());
        }
        Ok(FocusRoute::new(
            mount,
            instance,
            handle.clone(),
            registration.connection(),
            state.pane(),
        ))
    }

    /// Releases focus, pushing `false` to its holder. Returns whether a
    /// mount held it.
    fn clear_focus(&mut self) -> bool {
        match self.focus.clear() {
            Some(lost) => {
                self.push_focus(&lost, false);
                true
            }
            None => false,
        }
    }

    /// Pushes a `focus_changed` for `route` to its program; a closed
    /// connection drops it.
    fn push_focus(&self, route: &FocusRoute<P>, focused: bool) {
        let message = PushMsg::FocusChanged {
            handle: route.handle().clone(),
            instance: route.instance().to_string(),
            focused,
        };
        if let Err(error) = self.connections.push(route.connection(), &message) {
            tracing::debug!(%error, "a focus push was dropped");
        }
    }

    fn composited(&mut self, mount: MountId) -> WebviewHostResult<HostOutput<P>> {
        let route = {
            let (instance, handle, registration) = self.mount_owner(mount)?;
            if !registration.content().is_bridged() || self.composited.contains_key(&mount) {
                return Ok(HostOutput::default());
            }
            CompositeRoute {
                handle: handle.clone(),
                instance,
                connection: registration.connection(),
            }
        };
        self.push_compositing(&route, true);
        self.composited.insert(mount, route);
        Ok(HostOutput::default())
    }

    fn page_call(
        &mut self,
        mount: MountId,
        page_req: String,
        method: String,
        params: Value,
    ) -> HostOutput<P> {
        let mut output = HostOutput::default();
        let target = self.bridged_owner(mount);
        let refusal = match target {
            Err(_) => Some("no_owner"),
            Ok((instance, handle, connection)) => {
                let global = self.calls.mint();
                let call = PushMsg::Call {
                    handle,
                    instance: instance.to_string(),
                    req_id: global.clone(),
                    method,
                    params,
                };
                match self.connections.push(connection, &call) {
                    Ok(()) => {
                        self.calls.note(global, mount, page_req.clone(), connection);
                        None
                    }
                    Err(_) => Some("owner_unavailable"),
                }
            }
        };
        if let Some(error) = refusal {
            output.push_event(WebviewEvent::PageReply {
                mount,
                page_req,
                outcome: Err(error.into()),
            });
        }
        output
    }

    fn page_emit(
        &mut self,
        mount: MountId,
        event: String,
        payload: Value,
    ) -> WebviewHostResult<HostOutput<P>> {
        if event.is_empty() {
            return Err(Refusal::EmptyEventName.into());
        }
        let (_, handle, connection) = self.bridged_owner(mount)?;
        self.connections.push(
            connection,
            &PushMsg::Event {
                handle,
                event,
                payload,
            },
        )?;
        Ok(HostOutput::default())
    }

    fn url_changed(&mut self, mount: MountId, url: String) -> WebviewHostResult<HostOutput<P>> {
        let (instance, handle, connection) = self.bridged_owner(mount)?;
        let is_url = self
            .registry
            .get(&handle)
            .is_some_and(|registration| registration.content().is_url());
        if !is_url {
            return Err(Refusal::NotUrlView.into());
        }
        let call = PushMsg::Call {
            handle,
            instance: instance.to_string(),
            req_id: self.calls.mint(),
            method: "urlChanged".into(),
            params: json!({ "url": url }),
        };
        self.connections.push(connection, &call)?;
        Ok(HostOutput::default())
    }

    fn reply(
        &mut self,
        connection: ConnectionId,
        req_id: &str,
        ok: bool,
        value: Value,
        error: Option<String>,
    ) -> HostOutput<P> {
        let mut output = HostOutput::default();
        // NOTE: take_for_connection drops a reply whose sending connection
        // is not the one that originated the call, WITHOUT consuming the
        // pending entry — a foreign program replaying another connection's
        // (monotonic, guessable) global reqId must not settle or drop its call.
        if let Some((mount, page_req)) = self.calls.take_for_connection(req_id, connection) {
            let outcome = if ok {
                Ok(value)
            } else {
                Err(error.unwrap_or_default())
            };
            output.push_event(WebviewEvent::PageReply {
                mount,
                page_req,
                outcome,
            });
        }
        output
    }

    fn emit(
        &mut self,
        connection: ConnectionId,
        handle: &HandleId,
        event: String,
        payload: Value,
    ) -> WebviewHostResult<HostOutput<P>> {
        let registration = self.owned_registration(connection, handle)?;
        if !registration.content().is_bridged() {
            return Err(Refusal::NotBridged.into());
        }
        let mounts: Vec<MountId> = registration
            .instances()
            .iter()
            .filter_map(|instance| self.mounts.get(*instance))
            .map(MountState::mount)
            .collect();
        let mut output = HostOutput::default();
        for mount in mounts {
            output.push_event(WebviewEvent::PageEvent {
                mount,
                event: event.clone(),
                payload: payload.clone(),
            });
        }
        Ok(output)
    }

    fn navigate(
        &mut self,
        connection: ConnectionId,
        instance: &str,
        action: NavAction,
    ) -> WebviewHostResult<HostOutput<P>> {
        let (instance, _) = self.owned_instance(connection, instance)?;
        let mount = self
            .mounts
            .get(instance)
            .map(MountState::mount)
            .ok_or(Refusal::NotMounted)?;
        let navigation = match action {
            NavAction::To(url) => {
                let is_url = self
                    .registry
                    .resolve_instance(instance)
                    .is_some_and(|(_, registration)| registration.content().is_url());
                if !is_url {
                    return Err(Refusal::NotUrlView.into());
                }
                Navigation::To(validate_url(&url).map_err(|_| Refusal::InvalidNavigation)?)
            }
            NavAction::Back => Navigation::Back,
            NavAction::Forward => Navigation::Forward,
            NavAction::Reload => Navigation::Reload,
        };
        let mut output = HostOutput::default();
        output.push_event(WebviewEvent::Navigate { mount, navigation });
        Ok(output)
    }

    fn set_forward_keys(
        &mut self,
        connection: ConnectionId,
        handle: &HandleId,
        keys: Vec<ForwardChord>,
    ) -> WebviewHostResult<HostOutput<P>> {
        self.owned_registration(connection, handle)?;
        self.registry.replace_forward_keys(handle, keys.clone());
        let mut output = HostOutput::default();
        output.push_event(WebviewEvent::ForwardKeysChanged {
            handle: handle.clone(),
            keys,
        });
        Ok(output)
    }

    /// The placement, handle, and registration of `mount`, when `mount` is
    /// its placement's current mount.
    fn mount_owner(
        &self,
        mount: MountId,
    ) -> WebviewHostResult<(InstanceId, &HandleId, &Registration<P>)> {
        let (instance, _) = self.mounts.resolve(mount).ok_or(Refusal::StaleMount)?;
        let (handle, registration) = self
            .registry
            .resolve_instance(instance)
            .ok_or(Refusal::UnknownInstance)?;
        Ok((instance, handle, registration))
    }

    /// The placement, handle, and owning connection of `mount`, when it is
    /// current and its page has the bridge.
    fn bridged_owner(
        &self,
        mount: MountId,
    ) -> WebviewHostResult<(InstanceId, HandleId, ConnectionId)> {
        let (instance, handle, registration) = self.mount_owner(mount)?;
        if !registration.content().is_bridged() {
            return Err(Refusal::NotBridged.into());
        }
        Ok((instance, handle.clone(), registration.connection()))
    }

    /// Pushes a `compositing` for `route` to its program; a closed
    /// connection drops it.
    fn push_compositing(&self, route: &CompositeRoute, active: bool) {
        let message = PushMsg::Compositing {
            handle: route.handle.clone(),
            instance: route.instance.to_string(),
            active,
        };
        if let Err(error) = self.connections.push(route.connection, &message) {
            tracing::debug!(%error, "a compositing push was dropped");
        }
    }

    fn placement_mounted(
        &mut self,
        output: &mut HostOutput<P>,
        pane: P,
        instance: InstanceId,
        size: PlacementSize,
    ) {
        let owned = self
            .registry
            .resolve_instance(instance)
            .filter(|(_, registration)| registration.owner_pane() == pane);
        let Some((handle, registration)) = owned else {
            output.push_request(MuxRequest::RemovePlacements {
                pane,
                instances: vec![instance],
            });
            return;
        };
        match self.mounts.mount(instance, pane, size) {
            MountChange::New(mount) => output.push_event(WebviewEvent::Mounted {
                pane,
                mount,
                instance,
                spec: registration.content().mount_spec(handle, size),
            }),
            MountChange::Resized(mount) => output.push_event(WebviewEvent::Resized { mount, size }),
            MountChange::Unchanged => {}
        }
    }

    /// Ends the mounts of `instances` that sit in `pane`.
    fn end_pane_mounts(&mut self, output: &mut HostOutput<P>, pane: P, instances: &[InstanceId]) {
        let in_pane: Vec<InstanceId> = instances
            .iter()
            .copied()
            .filter(|instance| {
                self.mounts
                    .get(*instance)
                    .is_some_and(|state| state.pane() == pane)
            })
            .collect();
        self.end_mounts(output, &in_pane);
    }

    /// Ends the mounts of `instances` that are mounted: forgets their
    /// pending page calls, reports compositing stopped for those that
    /// composited, releases focus when one of them holds it, then reports
    /// them in one `Unmounted`.
    fn end_mounts(&mut self, output: &mut HostOutput<P>, instances: &[InstanceId]) {
        let ended: Vec<MountId> = instances
            .iter()
            .filter_map(|instance| self.mounts.remove(*instance))
            .map(|state| state.mount())
            .collect();
        if ended.is_empty() {
            return;
        }
        for mount in &ended {
            self.calls.drain_mount(*mount);
            if let Some(route) = self.composited.remove(mount) {
                self.push_compositing(&route, false);
            }
        }
        let focused_ended = self
            .focus
            .focused_mount()
            .is_some_and(|mount| ended.contains(&mount));
        if focused_ended && self.clear_focus() {
            output.push_event(WebviewEvent::FocusChanged { focused: None });
        }
        output.push_event(WebviewEvent::Unmounted { mounts: ended });
    }

    /// Tears released registrations down: ends their mounts, drops the
    /// reservations of every instance they minted while their pane still
    /// holds them, and stops serving their assets.
    fn release(
        &mut self,
        output: &mut HostOutput<P>,
        released: Vec<(HandleId, Registration<P>)>,
        reservations: Reservations,
    ) {
        for (handle, registration) in released {
            self.end_mounts(output, registration.instances());
            if reservations == Reservations::Held {
                output.push_request(MuxRequest::RemovePlacements {
                    pane: registration.owner_pane(),
                    instances: registration.instances().to_vec(),
                });
            }
            if registration.content().serves_asset() {
                output.push_event(WebviewEvent::AssetReleased { handle });
            }
        }
    }

    /// The live pane `connection` belongs to.
    fn live_pane_of(&self, connection: ConnectionId) -> WebviewHostResult<P> {
        let pane = self
            .connections
            .pane_of(connection)
            .ok_or(Refusal::ConnectionClosed)?;
        if self.tokens.is_live(pane) {
            Ok(pane)
        } else {
            Err(Refusal::OwnerGone.into())
        }
    }

    /// The registration of `handle`, when `connection` owns it.
    fn owned_registration(
        &self,
        connection: ConnectionId,
        handle: &HandleId,
    ) -> WebviewHostResult<&Registration<P>> {
        let registration = self.registry.get(handle).ok_or(Refusal::UnknownHandle)?;
        if registration.connection() == connection {
            Ok(registration)
        } else {
            Err(Refusal::NotOwner.into())
        }
    }

    /// Parses `spelled` and resolves it to an instance `connection` owns,
    /// with the pane that owns it.
    fn owned_instance(
        &self,
        connection: ConnectionId,
        spelled: &str,
    ) -> WebviewHostResult<(InstanceId, P)> {
        let instance: InstanceId = spelled.parse().map_err(|_| Refusal::MalformedInstance)?;
        let (_, registration) = self
            .registry
            .resolve_instance(instance)
            .ok_or(Refusal::UnknownInstance)?;
        if registration.connection() != connection {
            return Err(Refusal::NotOwner.into());
        }
        Ok((instance, registration.owner_pane()))
    }
}

/// Where a composited bridged mount's `compositing` pushes go, kept from its
/// first composite so the `active: false` push still reaches the program
/// after its registration is released.
struct CompositeRoute {
    handle: HandleId,
    instance: InstanceId,
    connection: ConnectionId,
}

/// Whether the pane of a released registration still holds its
/// reservations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reservations {
    /// The pane lives, so its reservations are dropped explicitly.
    Held,
    /// The pane closed and took its reservations along.
    Gone,
}

#[cfg(test)]
mod tests;
