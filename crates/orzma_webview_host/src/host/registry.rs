//! The registrations the host holds: each handle's content and owner, and
//! every instance minted for it.

use crate::boundary::{ForwardChord, HandleId};
use crate::control_socket::ConnectionId;
use crate::error::{Refusal, WebviewHostError, WebviewHostResult};
use crate::host::PaneKey;
use crate::host::validation::ValidatedRegistration;
use orzma_vt::prelude::InstanceId;
use std::collections::HashMap;

/// One live registration: its content, the pane and connection that own
/// it, and the instances minted for it in mint order.
pub(crate) struct Registration<P> {
    content: ValidatedRegistration,
    owner_pane: P,
    connection: ConnectionId,
    instances: Vec<InstanceId>,
}

impl<P: PaneKey> Registration<P> {
    /// A registration of `content` owned by `owner_pane` through
    /// `connection`, with `first` as its only instance.
    pub fn new(
        content: ValidatedRegistration,
        owner_pane: P,
        connection: ConnectionId,
        first: InstanceId,
    ) -> Self {
        Self {
            content,
            owner_pane,
            connection,
            instances: vec![first],
        }
    }

    /// The validated content.
    pub fn content(&self) -> &ValidatedRegistration {
        &self.content
    }

    /// The pane the registration belongs to.
    pub fn owner_pane(&self) -> P {
        self.owner_pane
    }

    /// The connection that registered it.
    pub fn connection(&self) -> ConnectionId {
        self.connection
    }

    /// Every instance minted for it, in mint order.
    pub fn instances(&self) -> &[InstanceId] {
        &self.instances
    }

    /// Refuses with `not_owner` unless `connection` registered it.
    pub fn check_owner(&self, connection: ConnectionId) -> WebviewHostResult {
        if self.connection == connection {
            Ok(())
        } else {
            Err(Refusal::NotOwner.into())
        }
    }
}

/// Maps each handle to its registration, and each minted instance back to
/// its handle.
///
/// # Invariants
///
/// An instance resolves exactly while its handle's registration lists it.
pub(crate) struct Registry<P> {
    by_handle: HashMap<HandleId, Registration<P>>,
    by_instance: HashMap<InstanceId, HandleId>,
}

impl<P: PaneKey> Registry<P> {
    /// An empty registry.
    pub fn new() -> Self {
        Self {
            by_handle: HashMap::new(),
            by_instance: HashMap::new(),
        }
    }

    /// The registration of `handle`, if live.
    pub fn get(&self, handle: &HandleId) -> Option<&Registration<P>> {
        self.by_handle.get(handle)
    }

    /// The handle and registration `instance` was minted for, if live.
    pub fn resolve_instance(&self, instance: InstanceId) -> Option<(&HandleId, &Registration<P>)> {
        let handle = self.by_instance.get(&instance)?;
        self.by_handle
            .get(handle)
            .map(|registration| (handle, registration))
    }

    /// Adds `registration` under `handle`.
    ///
    /// # Errors
    ///
    /// Returns [`WebviewHostError::DuplicateId`] when `handle`, or the
    /// registration's first instance, is already live.
    pub fn insert(&mut self, handle: HandleId, registration: Registration<P>) -> WebviewHostResult {
        let duplicate = self.by_handle.contains_key(&handle)
            || registration
                .instances
                .iter()
                .any(|instance| self.by_instance.contains_key(instance));
        if duplicate {
            return Err(WebviewHostError::DuplicateId);
        }
        for instance in &registration.instances {
            self.by_instance.insert(*instance, handle.clone());
        }
        self.by_handle.insert(handle, registration);
        Ok(())
    }

    /// Adds `instance` to the registration of `handle`.
    ///
    /// # Errors
    ///
    /// Returns [`Refusal::UnknownHandle`] when `handle` is not live, and
    /// [`WebviewHostError::DuplicateId`] when `instance` already is.
    pub fn add_instance(&mut self, handle: &HandleId, instance: InstanceId) -> WebviewHostResult {
        if self.by_instance.contains_key(&instance) {
            return Err(WebviewHostError::DuplicateId);
        }
        let registration = self
            .by_handle
            .get_mut(handle)
            .ok_or(Refusal::UnknownHandle)?;
        registration.instances.push(instance);
        self.by_instance.insert(instance, handle.clone());
        Ok(())
    }

    /// Replaces the forward-key chords of `handle`; an unknown handle is
    /// left alone.
    pub fn replace_forward_keys(&mut self, handle: &HandleId, keys: Vec<ForwardChord>) {
        if let Some(registration) = self.by_handle.get_mut(handle) {
            registration.content.set_forward_keys(keys);
        }
    }

    /// Removes the registration of `handle`, and every instance with it.
    pub fn remove(&mut self, handle: &HandleId) -> Option<Registration<P>> {
        let registration = self.by_handle.remove(handle)?;
        for instance in &registration.instances {
            self.by_instance.remove(instance);
        }
        Some(registration)
    }

    /// Removes every registration `connection` made.
    pub fn remove_by_connection(
        &mut self,
        connection: ConnectionId,
    ) -> Vec<(HandleId, Registration<P>)> {
        self.drain_where(|registration| registration.connection == connection)
    }

    /// Removes every registration `pane` owns.
    pub fn remove_by_pane(&mut self, pane: P) -> Vec<(HandleId, Registration<P>)> {
        self.drain_where(|registration| registration.owner_pane == pane)
    }

    fn drain_where(
        &mut self,
        pred: impl Fn(&Registration<P>) -> bool,
    ) -> Vec<(HandleId, Registration<P>)> {
        let handles: Vec<HandleId> = self
            .by_handle
            .iter()
            .filter(|(_, registration)| pred(registration))
            .map(|(handle, _)| handle.clone())
            .collect();
        handles
            .into_iter()
            .filter_map(|handle| {
                self.remove(&handle)
                    .map(|registration| (handle, registration))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::RegisterKind;

    fn content() -> ValidatedRegistration {
        ValidatedRegistration::try_from(RegisterKind::Inline {
            html: "<p>x</p>".into(),
            interactive: true,
            forward_keys: vec![],
            preload: vec![],
        })
        .expect("a valid inline registration")
    }

    fn registration(pane: u32, connection: u64, first: u128) -> Registration<u32> {
        Registration::new(
            content(),
            pane,
            ConnectionId::new(connection),
            InstanceId(first),
        )
    }

    /// Asserts that an inserted registration resolves by handle and by each
    /// of its instances, and that removing it takes every instance along.
    ///
    /// Case: a program registers a view, asks for a second placement, then
    /// unregisters.
    #[test]
    fn instances_resolve_while_their_registration_lives() {
        let mut registry = Registry::new();
        let handle = HandleId::from("h");
        registry
            .insert(handle.clone(), registration(1, 7, 1))
            .unwrap();
        registry.add_instance(&handle, InstanceId(2)).unwrap();
        assert_eq!(
            registry.resolve_instance(InstanceId(1)).map(|(h, _)| h),
            Some(&handle)
        );
        assert_eq!(
            registry.resolve_instance(InstanceId(2)).map(|(h, _)| h),
            Some(&handle)
        );
        let removed = registry.remove(&handle).expect("the handle was live");
        assert_eq!(removed.instances(), [InstanceId(1), InstanceId(2)]);
        assert!(registry.resolve_instance(InstanceId(1)).is_none());
        assert!(registry.resolve_instance(InstanceId(2)).is_none());
    }

    /// Asserts that a handle or instance that is already live is refused
    /// as a duplicate, and an instance for an unknown handle as unknown.
    ///
    /// Case: a broken random source repeats itself.
    #[test]
    fn duplicates_and_unknown_handles_are_refused() {
        let mut registry = Registry::new();
        let handle = HandleId::from("h");
        registry
            .insert(handle.clone(), registration(1, 7, 1))
            .unwrap();
        assert!(matches!(
            registry.insert(handle.clone(), registration(1, 7, 9)),
            Err(WebviewHostError::DuplicateId)
        ));
        assert!(matches!(
            registry.insert(HandleId::from("other"), registration(1, 7, 1)),
            Err(WebviewHostError::DuplicateId)
        ));
        assert!(matches!(
            registry.add_instance(&handle, InstanceId(1)),
            Err(WebviewHostError::DuplicateId)
        ));
        assert!(matches!(
            registry.add_instance(&HandleId::from("nope"), InstanceId(5)),
            Err(WebviewHostError::Refused(Refusal::UnknownHandle))
        ));
    }

    /// Asserts that removing by connection or by pane takes exactly the
    /// registrations that connection or pane owns.
    ///
    /// Case: one program disconnects while another keeps its view, and
    /// later a pane closes.
    #[test]
    fn removal_by_connection_or_pane_takes_only_theirs() {
        let mut registry = Registry::new();
        registry
            .insert(HandleId::from("a"), registration(1, 7, 1))
            .unwrap();
        registry
            .insert(HandleId::from("b"), registration(1, 7, 2))
            .unwrap();
        registry
            .insert(HandleId::from("c"), registration(2, 8, 3))
            .unwrap();
        let mut by_connection: Vec<_> = registry
            .remove_by_connection(ConnectionId::new(7))
            .into_iter()
            .map(|(handle, _)| handle)
            .collect();
        by_connection.sort_by(|a, b| a.as_str().cmp(b.as_str()));
        assert_eq!(by_connection, [HandleId::from("a"), HandleId::from("b")]);
        assert!(registry.get(&HandleId::from("c")).is_some());
        let by_pane = registry.remove_by_pane(2);
        assert_eq!(by_pane.len(), 1);
        assert!(registry.get(&HandleId::from("c")).is_none());
    }
}
