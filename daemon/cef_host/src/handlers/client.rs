//! CEF ClientHandler aggregator — exposes RenderHandler + LifeSpanHandler to
//! a `browser_host_create_browser_sync` call.

use cef::rc::Rc as _;
use cef::{Client, ImplClient, LifeSpanHandler, RenderHandler, WrapClient, wrap_client};

wrap_client! {
    pub struct OzmuxClient {
        render: RenderHandler,
        life_span: LifeSpanHandler,
    }

    impl Client {
        fn render_handler(&self) -> Option<RenderHandler> {
            Some(self.render.clone())
        }

        fn life_span_handler(&self) -> Option<LifeSpanHandler> {
            Some(self.life_span.clone())
        }
    }
}
