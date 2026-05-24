//! `TerminalRenderBundle` — a render-side helper bundling `TerminalGrid` and
//! `MaterialNode<TerminalUiMaterial>` into one type. `TerminalMaterialState`
//! is attached separately by `TerminalMaterialStatePlugin`'s component hook
//! when the `MaterialNode` is inserted, so it does not belong in this bundle.

use crate::material::TerminalUiMaterial;
use crate::schema::TerminalGrid;
use bevy::asset::Handle;
use bevy::prelude::*;

/// Components needed to render a terminal, packaged as one Bundle.
#[derive(Bundle, Default)]
pub struct TerminalRenderBundle {
    pub grid: TerminalGrid,
    pub material: MaterialNode<TerminalUiMaterial>,
}

impl TerminalRenderBundle {
    /// Returns a bundle wrapping the given material handle.
    pub fn new(material: Handle<TerminalUiMaterial>) -> Self {
        Self {
            grid: TerminalGrid::default(),
            material: MaterialNode(material),
        }
    }
}
