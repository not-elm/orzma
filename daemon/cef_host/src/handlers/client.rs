//! CEF ClientHandler aggregator — exposes RenderHandler, LifeSpanHandler,
//! DisplayHandler, and LoadHandler to a `browser_host_create_browser_sync` call.

use cef::rc::Rc as _;
use cef::{
    Client, DisplayHandler, ImplClient, LifeSpanHandler, LoadHandler, RenderHandler, WrapClient,
    wrap_client,
};

wrap_client! {
    pub struct OzmuxClient {
        render: RenderHandler,
        life_span: LifeSpanHandler,
        display: DisplayHandler,
        load: LoadHandler,
    }

    impl Client {
        fn render_handler(&self) -> Option<RenderHandler> {
            Some(self.render.clone())
        }

        fn life_span_handler(&self) -> Option<LifeSpanHandler> {
            Some(self.life_span.clone())
        }

        fn display_handler(&self) -> Option<DisplayHandler> {
            Some(self.display.clone())
        }

        fn load_handler(&self) -> Option<LoadHandler> {
            Some(self.load.clone())
        }
    }
}
