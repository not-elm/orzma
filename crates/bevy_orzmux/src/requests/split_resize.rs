//! The divider movement the host UI asks the backend to apply, sent as
//! `OrzmuxCommand::ResizeSplit`.

use crate::OrzmuxConnection;
use bevy::prelude::*;
use orzmux::prelude::{OrzmuxCommand, SplitId};

/// Fired by the host UI to move a split's divider.
#[derive(Event, Debug, Clone, Copy)]
pub struct RequestSplitResize {
    /// The split whose divider moves.
    pub split: SplitId,
    /// The whole-window cell boundary to put the divider on: `x` for
    /// a vertical split, `y` for a horizontal one.
    pub position: u16,
}

pub(super) struct SplitResizePlugin;

impl Plugin for SplitResizePlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(apply_split_resize.run_if(resource_exists::<OrzmuxConnection>));
    }
}

fn apply_split_resize(e: On<RequestSplitResize>, connection: Res<OrzmuxConnection>) {
    connection.0.send(OrzmuxCommand::ResizeSplit {
        split: e.split,
        position: e.position,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::requests::test_support::{app_with_connection, sent};

    /// Asserts that a divider-move request becomes a `ResizeSplit`
    /// command carrying the same split and position.
    ///
    /// Case: the user drags a divider one cell to the right.
    #[test]
    fn a_split_resize_request_sends_a_resize_command() {
        let (mut app, commands) = app_with_connection(SplitResizePlugin);

        app.world_mut().trigger(RequestSplitResize {
            split: SplitId(3),
            position: 47,
        });
        app.update();

        let commands = sent(&commands);
        let [OrzmuxCommand::ResizeSplit { split, position }] = commands.as_slice() else {
            panic!("expected one ResizeSplit");
        };
        assert_eq!(*split, SplitId(3));
        assert_eq!(*position, 47);
    }
}
