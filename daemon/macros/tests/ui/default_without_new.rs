use ozmux_macros::NewType;

#[derive(NewType)]
#[newtype(default)]
pub struct DefaultWithoutNew(String);

fn main() {}
