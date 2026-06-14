//! Reconciles the [`ProjectionModel`] into ECS entities, maintaining the
//! tmux-id → entity index.

use crate::components::{TmuxPane, TmuxWindow};
use crate::model::ProjectionModel;
use bevy::prelude::*;
use std::collections::{HashMap, HashSet};
use tmux_control_parser::{PaneId, WindowId};

/// Maps tmux ids to their projected entities.
#[derive(Resource, Default)]
pub struct TmuxProjection {
    /// Window id → entity.
    pub windows: HashMap<WindowId, Entity>,
    /// Pane id → entity.
    pub panes: HashMap<PaneId, Entity>,
}

/// Spawns/updates/despawns `TmuxWindow`/`TmuxPane` entities so they match the
/// current [`ProjectionModel`]. Runs only when the model changed.
pub(crate) fn reconcile_projection(
    mut commands: Commands,
    mut index: ResMut<TmuxProjection>,
    model: Res<ProjectionModel>,
) {
    if !model.is_changed() {
        return;
    }
    reconcile_windows(&mut commands, &mut index, &model);
}

fn reconcile_windows(commands: &mut Commands, index: &mut TmuxProjection, model: &ProjectionModel) {
    let live_windows: HashSet<WindowId> = model.windows.iter().map(|w| w.id).collect();
    let live_panes: HashSet<PaneId> = model
        .windows
        .iter()
        .flat_map(|w| w.panes.iter().map(|p| p.id))
        .collect();

    index.windows.retain(|id, entity| {
        let keep = live_windows.contains(id);
        if !keep {
            commands.entity(*entity).despawn();
        }
        keep
    });
    index.panes.retain(|id, entity| {
        let keep = live_panes.contains(id);
        if !keep {
            commands.entity(*entity).despawn();
        }
        keep
    });

    for window in &model.windows {
        match index.windows.get(&window.id) {
            Some(entity) => {
                commands.entity(*entity).insert(TmuxWindow {
                    id: window.id,
                    active: window.active,
                    name: window.name.clone(),
                });
            }
            None => {
                let entity = commands
                    .spawn(TmuxWindow {
                        id: window.id,
                        active: window.active,
                        name: window.name.clone(),
                    })
                    .id();
                index.windows.insert(window.id, entity);
            }
        }
    }
    for window in &model.windows {
        for pane in &window.panes {
            match index.panes.get(&pane.id) {
                Some(entity) => {
                    commands.entity(*entity).insert(TmuxPane {
                        id: pane.id,
                        dims: pane.dims,
                    });
                }
                None => {
                    let entity = commands
                        .spawn(TmuxPane {
                            id: pane.id,
                            dims: pane.dims,
                        })
                        .id();
                    index.panes.insert(pane.id, entity);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{PaneModel, WindowModel};
    use tmux_control_parser::CellDims;

    fn dims() -> CellDims {
        CellDims {
            width: 80,
            height: 24,
            xoff: 0,
            yoff: 0,
        }
    }

    fn app() -> App {
        let mut app = App::new();
        app.init_resource::<ProjectionModel>();
        app.init_resource::<TmuxProjection>();
        app.add_systems(Update, reconcile_projection);
        app
    }

    #[test]
    fn spawns_window_and_pane_entities() {
        let mut app = app();
        app.world_mut().resource_mut::<ProjectionModel>().windows = vec![WindowModel {
            id: WindowId(1),
            active: true,
            name: "main".to_string(),
            panes: vec![PaneModel {
                id: PaneId(9),
                dims: dims(),
            }],
        }];
        app.update();

        let index = app.world().resource::<TmuxProjection>();
        assert_eq!(index.windows.len(), 1);
        assert_eq!(index.panes.len(), 1);
        let pane_entity = index.panes[&PaneId(9)];
        let pane = app.world().get::<TmuxPane>(pane_entity).unwrap();
        assert_eq!(pane.id, PaneId(9));
    }

    #[test]
    fn despawns_removed_window_and_its_panes() {
        let mut app = app();
        app.world_mut().resource_mut::<ProjectionModel>().windows = vec![WindowModel {
            id: WindowId(1),
            active: true,
            name: "main".to_string(),
            panes: vec![PaneModel {
                id: PaneId(9),
                dims: dims(),
            }],
        }];
        app.update();
        app.world_mut()
            .resource_mut::<ProjectionModel>()
            .windows
            .clear();
        app.update();

        let index = app.world().resource::<TmuxProjection>();
        assert!(index.windows.is_empty());
        assert!(index.panes.is_empty());
    }
}
