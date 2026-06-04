//! Surface tab-title sync with the CEF webview page title.

use bevy::prelude::*;

/// The live webview page title for a browser/extension surface, mirrored from
/// bevy_cef's `WebviewTitle` onto the multiplexer Surface entity so `tab_label`
/// and the chrome-dirty hook can read it like `Cwd`.
#[derive(Component, Debug, Clone, Default)]
pub(crate) struct WebTitle(pub(crate) String);
