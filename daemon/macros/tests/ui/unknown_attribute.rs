use ozmux_macros::NewType;

#[derive(NewType)]
#[newtype(unknown_key)]
pub struct UnknownAttr(String);

fn main() {}
