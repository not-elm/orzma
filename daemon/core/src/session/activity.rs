use crate::define_string_new_type;

pub struct Activity {
    pub name: String,
}

define_string_new_type!(ActivityId);
