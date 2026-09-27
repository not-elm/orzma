//! The page calls the host forwarded to a program and still waits on a
//! reply for.

use crate::boundary::MountId;
use crate::control_socket::ConnectionId;
use std::collections::HashMap;

/// Every forwarded page call awaiting its reply, by global reqId, and the
/// counter that mints those ids.
#[derive(Default)]
pub(crate) struct InFlightCalls {
    calls: HashMap<String, PendingCall>,
    next_id: u64,
}

impl InFlightCalls {
    /// Mints the next global reqId: a decimal counter starting at `"0"`,
    /// shared by every connection and therefore guessable.
    pub fn mint(&mut self) -> String {
        let id = self.next_id.to_string();
        self.next_id = self.next_id.wrapping_add(1);
        id
    }

    /// Records that the call `global_id`, made by the page of `mount` as
    /// `page_req`, was forwarded to `connection`.
    pub fn note(
        &mut self,
        global_id: String,
        mount: MountId,
        page_req: String,
        connection: ConnectionId,
    ) {
        self.calls.insert(
            global_id,
            PendingCall {
                mount,
                page_req,
                connection,
            },
        );
    }

    /// Takes the call `global_id` when `connection` is the one it was
    /// forwarded to, returning its mount and the page's id. A reply from
    /// any other connection leaves the call pending and returns `None`.
    pub fn take_for_connection(
        &mut self,
        global_id: &str,
        connection: ConnectionId,
    ) -> Option<(MountId, String)> {
        match self.calls.get(global_id) {
            Some(call) if call.connection == connection => self
                .calls
                .remove(global_id)
                .map(|call| (call.mount, call.page_req)),
            _ => None,
        }
    }

    /// Takes every call forwarded to `connection`.
    pub fn drain_connection(&mut self, connection: ConnectionId) -> Vec<(MountId, String)> {
        self.calls
            .extract_if(|_, call| call.connection == connection)
            .map(|(_, call)| (call.mount, call.page_req))
            .collect()
    }

    /// Forgets every call the page of `mount` made.
    pub fn drain_mount(&mut self, mount: MountId) {
        self.calls.retain(|_, call| call.mount != mount);
    }
}

/// One forwarded call.
struct PendingCall {
    mount: MountId,
    page_req: String,
    connection: ConnectionId,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Asserts that minted ids count up from `"0"`.
    ///
    /// Case: two pages call their programs one after the other.
    #[test]
    fn minted_ids_count_up_from_zero() {
        let mut calls = InFlightCalls::default();
        assert_eq!(calls.mint(), "0");
        assert_eq!(calls.mint(), "1");
    }

    /// Asserts that only the connection a call was forwarded to can take
    /// it, and a foreign attempt leaves it pending.
    ///
    /// Case: a hostile program replays another program's guessable reqId
    /// before the real reply arrives.
    #[test]
    fn only_the_owning_connection_takes_a_call() {
        let mut calls = InFlightCalls::default();
        calls.note(
            "0".into(),
            MountId::new(1),
            "p0".into(),
            ConnectionId::new(7),
        );
        assert_eq!(calls.take_for_connection("0", ConnectionId::new(8)), None);
        assert_eq!(
            calls.take_for_connection("0", ConnectionId::new(7)),
            Some((MountId::new(1), "p0".into()))
        );
        assert_eq!(calls.take_for_connection("0", ConnectionId::new(7)), None);
    }

    /// Asserts that draining by connection takes that connection's calls and
    /// draining by mount forgets that mount's calls, leaving the rest.
    ///
    /// Case: one program disconnects and another program's page is
    /// unmounted while both have calls in flight.
    #[test]
    fn draining_takes_only_the_named_calls() {
        let mut calls = InFlightCalls::default();
        calls.note(
            "0".into(),
            MountId::new(1),
            "p0".into(),
            ConnectionId::new(7),
        );
        calls.note(
            "1".into(),
            MountId::new(2),
            "p1".into(),
            ConnectionId::new(8),
        );
        calls.note(
            "2".into(),
            MountId::new(3),
            "p2".into(),
            ConnectionId::new(8),
        );
        assert_eq!(
            calls.drain_connection(ConnectionId::new(7)),
            [(MountId::new(1), "p0".to_string())]
        );
        calls.drain_mount(MountId::new(2));
        assert_eq!(calls.take_for_connection("1", ConnectionId::new(8)), None);
        assert!(
            calls
                .take_for_connection("2", ConnectionId::new(8))
                .is_some()
        );
    }
}
