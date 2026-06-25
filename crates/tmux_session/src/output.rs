//! The `%output` projection seam: the `PaneOutput` message and the pure
//! helper that extracts pane output from a drained transport batch.

use bevy::prelude::*;
use tmux_control::{ClientEvent, ControlEvent, TransportEvent};
use tmux_control_parser::PaneId;

/// One batch of bytes tmux emitted for a pane (`%output`). Written by the
/// drain system and consumed by the binary's render layer, which maps
/// `pane` to its `TmuxPane` entity.
#[derive(Message, Debug, Clone, PartialEq, Eq)]
pub struct PaneOutput {
    /// tmux pane id (`%N`) the bytes belong to.
    pub pane: PaneId,
    /// Raw VT bytes from `%output`.
    pub data: Vec<u8>,
}

/// Request from the renderer layer (the binary) to re-`capture-pane`-seed a
/// pane whose grid was left structurally unpainted after a layout change. The
/// crate handles it via `request_pane_capture`, reusing the existing reply
/// correlation and in-flight suppression.
#[derive(Message)]
pub struct RequestPaneReseed {
    /// tmux pane (`%N`) to re-seed.
    pub pane: PaneId,
}

/// Extracts a [`PaneOutput`] for every `%output` or `%extended-output`
/// notification in a drained transport batch, preserving stream order.
pub(crate) fn collect_pane_outputs(events: &[TransportEvent]) -> Vec<PaneOutput> {
    events
        .iter()
        .filter_map(|event| match event {
            TransportEvent::Protocol(ClientEvent::Notification(ControlEvent::Output {
                pane,
                data,
            })) => Some(PaneOutput {
                pane: *pane,
                data: data.clone(),
            }),
            TransportEvent::Protocol(ClientEvent::Notification(ControlEvent::ExtendedOutput {
                pane,
                data,
                ..
            })) => Some(PaneOutput {
                pane: *pane,
                data: data.clone(),
            }),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tmux_control_parser::WindowId;

    #[test]
    fn collects_output_events_in_order_and_skips_others() {
        let events = vec![
            TransportEvent::Protocol(ClientEvent::Notification(ControlEvent::WindowAdd {
                window: WindowId(1),
            })),
            TransportEvent::Protocol(ClientEvent::Notification(ControlEvent::Output {
                pane: PaneId(1),
                data: vec![b'a'],
            })),
            TransportEvent::Protocol(ClientEvent::Notification(ControlEvent::ExtendedOutput {
                pane: PaneId(3),
                age: 50,
                data: vec![b'x', b'y'],
            })),
            TransportEvent::Protocol(ClientEvent::Notification(ControlEvent::Output {
                pane: PaneId(2),
                data: vec![b'b', b'c'],
            })),
        ];
        let out = collect_pane_outputs(&events);
        assert_eq!(
            out,
            vec![
                PaneOutput {
                    pane: PaneId(1),
                    data: vec![b'a'],
                },
                PaneOutput {
                    pane: PaneId(3),
                    data: vec![b'x', b'y'],
                },
                PaneOutput {
                    pane: PaneId(2),
                    data: vec![b'b', b'c'],
                },
            ]
        );
    }
}
