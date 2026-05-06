use ozmux_macros::NewType;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize, NewType)]
#[newtype(as_ref(str), display, new(uuid_v4_string), default)]
pub struct WindowId(String);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_window_ids_are_distinct() {
        let a = WindowId::new();
        let b = WindowId::new();
        assert_ne!(a, b);
    }

    #[test]
    fn window_id_displays_as_inner_string() {
        let id = WindowId::new();
        let s: String = id.as_ref().to_string();
        assert!(!s.is_empty());
    }
}
