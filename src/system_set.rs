use bevy::prelude::*;

/// The label enum labeling the types of systems in Orzma
#[derive(Debug, Hash, PartialEq, Eq, Clone, SystemSet)]
pub enum OrzmaSystems {
    /// Per-frame input handling — keyboard and mouse. Members run in
    /// `crate::input::InputPhase` order: `Hover` → `Dispatch` →
    /// `FocusedKey`. The chain ensures click-to-focus (Dispatch)
    /// retargets the focused surface before the keyboard chord
    /// dispatcher (FocusedKey) reads it in the same frame.
    Input,
}
