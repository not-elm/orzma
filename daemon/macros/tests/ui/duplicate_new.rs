use ozmux_macros::NewType;

#[derive(NewType)]
#[newtype(new(uuid_v4_string), new(default))]
pub struct DuplicateNew(String);

fn main() {}
