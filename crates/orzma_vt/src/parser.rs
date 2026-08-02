use vtparse::VTActor;

use crate::vt_state::VTState;

pub trait VTHandler {
    fn print(&mut self, b: char);
}

pub(crate) struct Parser<'w, H: VTHandler> {
    pub handler: &'w mut H,
    pub state: &'w mut ParserState,
}

impl<H: VTHandler> VTActor for Parser<'_, H> {
    fn print(&mut self, b: char) {
        self.handler.print(b);
    }

    fn execute_c0_or_c1(&mut self, control: u8) {
        todo!()
    }

    fn dcs_hook(
        &mut self,
        mode: u8,
        params: &[i64],
        intermediates: &[u8],
        ignored_excess_intermediates: bool,
    ) {
        todo!()
    }

    fn dcs_put(&mut self, byte: u8) {
        todo!()
    }

    fn dcs_unhook(&mut self) {
        todo!()
    }

    fn esc_dispatch(
        &mut self,
        params: &[i64],
        intermediates: &[u8],
        ignored_excess_intermediates: bool,
        byte: u8,
    ) {
        todo!()
    }

    fn csi_dispatch(&mut self, params: &[vtparse::CsiParam], parameters_truncated: bool, byte: u8) {
        todo!()
    }

    fn osc_dispatch(&mut self, params: &[&[u8]]) {
        todo!()
    }

    fn apc_dispatch(&mut self, data: Vec<u8>) {
        todo!()
    }
}

pub(crate) struct ParserState {}
