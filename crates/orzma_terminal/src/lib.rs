use crate::{coalescer::Coalescer, error::OrzmaTermResult, event::TermEvent};
use orzma_vt::prelude::*;
use std::path::PathBuf;

mod coalescer;
mod error;
mod event;
mod input;

pub mod prelude {
    pub use crate::{OrzmaTerm, error::*};
}

/// Spawn parameters consumed exactly once by `TerminalBundle::spawn`.
pub struct SpawnOptions {
    /// Terminal column count.
    pub cols: u16,
    /// Terminal row count.
    pub rows: u16,
    /// Shell program to launch (absolute path or `$PATH`-resolvable name).
    pub shell: String,
    /// Initial working directory for the spawned shell.
    pub cwd: Option<PathBuf>,
    /// Arbitrary environment variables forwarded to the shell.
    pub env: Vec<(EnvKey, EnvValue)>,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct EnvKey(pub String);

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct EnvValue(pub String);

pub struct OrzmaTerm<V: OrzmaVt> {
    vt: V,
    coalescer: Coalescer,
}

impl<V: OrzmaVt> OrzmaTerm<V> {
    pub fn spawn(options: SpawnOptions) -> OrzmaTermResult<Self> {
        todo!("OrzmaTerm::spawn")
    }

    /// HACK:
    /// VecでTermEventを収集しているが、この関数はほぼ米フレームで呼ばれることが予想されるため、
    /// コールバック形式などにしたほうがいい？
    pub fn pump(&mut self) -> Vec<TermEvent> {
        todo!("OrzmaTerm::pump")
    }

    pub fn resize(&mut self, cols: u16, rows: u16) -> OrzmaTermResult {
        todo!("OrzmaTerm::resize")
    }

    pub fn write_mouse_input(&mut self) {}
}
