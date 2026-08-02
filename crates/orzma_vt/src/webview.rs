//! Webview APC capture: feeds `apc_dispatch` payloads through
//! [`ApcWebviewVerb::parse`] and holds the parsed verb.

use vtparse::{self, VTActor};

mod verb;

pub use verb::ApcWebviewVerb;

pub struct WebviewApcState {
    verb: Option<ApcWebviewVerb>,
}

impl VTActor for WebviewApcState {
    fn print(&mut self, b: char) {
        // ignore
    }

    fn execute_c0_or_c1(&mut self, control: u8) {
        // ignore
    }

    fn dcs_hook(
        &mut self,
        mode: u8,
        params: &[i64],
        intermediates: &[u8],
        ignored_excess_intermediates: bool,
    ) {
        // ignore
    }

    fn dcs_put(&mut self, byte: u8) {
        // ignore
    }

    fn dcs_unhook(&mut self) {
        // ignore
    }

    fn esc_dispatch(
        &mut self,
        params: &[i64],
        intermediates: &[u8],
        ignored_excess_intermediates: bool,
        byte: u8,
    ) {
        // ignore
    }

    fn csi_dispatch(&mut self, params: &[vtparse::CsiParam], parameters_truncated: bool, byte: u8) {
        // ignore
    }

    fn osc_dispatch(&mut self, params: &[&[u8]]) {
        // ignore
    }

    fn apc_dispatch(&mut self, data: Vec<u8>) {
        if let Some(verb) = ApcWebviewVerb::parse(&data) {
            self.verb.replace(verb);
        }
    }
}
