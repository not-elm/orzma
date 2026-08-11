mod color;
mod cursor;
mod damage;
mod error;
mod extension;
mod frame;
mod hyperlink;
mod modes;
mod run;
mod scroll;
mod selection;
mod signal;
mod vi;
mod vt;

pub mod prelude {
    pub use crate::{
        color::*, cursor::*, damage::DamageVerdict, error::*, extension::*, hyperlink::*, modes::*,
        run::*, scroll::*, selection::*, signal::*, vi::*, vt::*,
    };
}
