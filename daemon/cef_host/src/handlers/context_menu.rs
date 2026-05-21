//! cef_host context menu handler. Clears the model so CEF does not show its
//! native OS context menu — the frontend draws its own React menu on
//! right-click instead.

use cef::rc::Rc as _;
use cef::{
    Browser, ContextMenuHandler, ContextMenuParams, Frame, ImplContextMenuHandler, ImplMenuModel,
    MenuModel, WrapContextMenuHandler, wrap_context_menu_handler,
};

wrap_context_menu_handler! {
    pub struct OzmuxContextMenuHandler;

    impl ContextMenuHandler {
        fn on_before_context_menu(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _params: Option<&mut ContextMenuParams>,
            model: Option<&mut MenuModel>,
        ) {
            // NOTE: clearing the model suppresses the OS context menu so the
            // frontend can draw its own React ContextMenu instead.
            if let Some(m) = model {
                m.clear();
            }
        }
    }
}
