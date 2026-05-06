use ozmux_macros::NewType;

#[derive(NewType)]
#[newtype(as_ref(str), as_ref(str))]
pub struct DuplicateAsRef(String);

fn main() {}
