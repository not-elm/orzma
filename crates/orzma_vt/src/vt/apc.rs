//! Webview APC capture: feeds `apc_dispatch` payloads through
//! [`ApcWebviewVerb::parse`] and holds the parsed verb.

use crate::schema::ApcWebviewVerb;
use vtparse::{self, VTActor};

#[derive(Default)]
pub(crate) struct ApcState {
    webview: Option<ApcWebviewVerb>,
}

impl VTActor for ApcState {
    fn print(&mut self, _b: char) {
        // ignore
    }

    fn execute_c0_or_c1(&mut self, _control: u8) {
        // ignore
    }

    fn dcs_hook(
        &mut self,
        _mode: u8,
        _params: &[i64],
        _intermediates: &[u8],
        _ignored_excess_intermediates: bool,
    ) {
        // ignore
    }

    fn dcs_put(&mut self, _byte: u8) {
        // ignore
    }

    fn dcs_unhook(&mut self) {
        // ignore
    }

    fn esc_dispatch(
        &mut self,
        _params: &[i64],
        _intermediates: &[u8],
        _ignored_excess_intermediates: bool,
        _byte: u8,
    ) {
        // ignore
    }

    fn csi_dispatch(
        &mut self,
        _params: &[vtparse::CsiParam],
        _parameters_truncated: bool,
        _byte: u8,
    ) {
        // ignore
    }

    fn osc_dispatch(&mut self, _params: &[&[u8]]) {
        // ignore
    }

    fn apc_dispatch(&mut self, data: Vec<u8>) {
        if let Some(verb) = ApcWebviewVerb::parse(&data) {
            self.webview.replace(verb);
        }
    }
}
