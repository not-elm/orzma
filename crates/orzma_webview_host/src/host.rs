//! The webview host: every client's state and every decision about it,
//! driven by control-socket events, VT placement signals, and pane
//! lifecycle changes.

use crate::boundary::{HandleId, MountId, WebviewEvent};
use crate::control_socket::{ConnectionId, ControlEvent, ControlSocket};
use crate::error::{Refusal, RegisterError, WebviewHostResult};
use crate::host::connections::Connections;
use crate::host::mint::mint_instance_id;
use crate::host::mounts::{MountChange, Mounts};
use crate::host::registry::{Registration, Registry};
use crate::host::tokens::Tokens;
use crate::protocol::ServerMsg;
use crossbeam_channel::{Receiver, Sender, TryRecvError};
use orzma_vt::prelude::{
    GridColumn, InstanceId, MAX_COLS, MAX_ROWS, PlacementSize, ScreenLine, VtSignal,
};
use std::fmt::Debug;
use std::hash::Hash;

mod connections;
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
    /// connection does not own, an instance not spelled as 32 hex digits, or
    /// a mount size out of range. Nothing changes then.
    pub fn control(&mut self, event: ControlEvent) -> WebviewHostResult<HostOutput<P>> {
        match event {
            ControlEvent::Hello {
                connection,
                token,
                writer: _,
                reply,
            } => {
                self.hello(connection, &token, &reply);
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
        }
    }

    fn new(socket: Option<ControlSocket>) -> Self {
        Self {
            socket,
            connections: Connections::new(),
            tokens: Tokens::new(),
            registry: Registry::new(),
            mounts: Mounts::new(),
        }
    }

    fn hello(&mut self, connection: ConnectionId, token: &str, reply: &Sender<bool>) {
        let pane = self.tokens.resolve(token);
        if let Some(pane) = pane {
            self.connections.insert(connection, pane);
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
            Err(error) => ServerMsg::err(error.wire_code()),
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

    /// Ends the mounts of `instances` that are mounted, reporting them in one
    /// `Unmounted`.
    fn end_mounts(&mut self, output: &mut HostOutput<P>, instances: &[InstanceId]) {
        let ended: Vec<MountId> = instances
            .iter()
            .filter_map(|instance| self.mounts.remove(*instance))
            .map(|state| state.mount())
            .collect();
        if !ended.is_empty() {
            output.push_event(WebviewEvent::Unmounted { mounts: ended });
        }
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
