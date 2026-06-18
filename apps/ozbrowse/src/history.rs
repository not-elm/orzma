//! URL navigation history: back and forward stacks.

/// Tracks visited URLs as two stacks so the user can navigate back and forward.
#[derive(Debug)]
pub(crate) struct History {
    back: Vec<String>,
    forward: Vec<String>,
}

impl History {
    pub(crate) fn new() -> Self {
        Self {
            back: Vec::new(),
            forward: Vec::new(),
        }
    }

    /// Pushes `current` onto the back stack, clears forward, and returns `new_url`.
    pub(crate) fn navigate(&mut self, current: String, new_url: String) -> String {
        self.back.push(current);
        self.forward.clear();
        new_url
    }

    /// Pops from back; pushes `current` onto forward. Returns the popped URL, or `None`.
    pub(crate) fn back(&mut self, current: String) -> Option<String> {
        let prev = self.back.pop()?;
        self.forward.push(current);
        Some(prev)
    }

    /// Pops from forward; pushes `current` onto back. Returns the popped URL, or `None`.
    pub(crate) fn forward(&mut self, current: String) -> Option<String> {
        let next = self.forward.pop()?;
        self.back.push(current);
        Some(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_history_is_empty() {
        let mut h = History::new();
        assert_eq!(h.back("x".into()), None);
        assert_eq!(h.forward("x".into()), None);
    }

    #[test]
    fn navigate_pushes_current_onto_back_and_clears_forward() {
        let mut h = History::new();
        h.navigate("a".into(), "b".into());
        assert!(h.back("b".into()).is_some());
    }

    #[test]
    fn navigate_returns_new_url() {
        let mut h = History::new();
        let result = h.navigate("a".into(), "b".into());
        assert_eq!(result, "b");
    }

    #[test]
    fn back_returns_none_when_empty() {
        let mut h = History::new();
        assert_eq!(h.back("x".into()), None);
    }

    #[test]
    fn forward_returns_none_when_empty() {
        let mut h = History::new();
        assert_eq!(h.forward("x".into()), None);
    }

    #[test]
    fn back_pops_from_back_and_pushes_to_forward() {
        let mut h = History::new();
        h.navigate("a".into(), "b".into()); // back=[a]
        let prev = h.back("b".into()).unwrap();
        assert_eq!(prev, "a");
        assert_eq!(h.back("a".into()), None);
        assert!(h.forward("a".into()).is_some());
    }

    #[test]
    fn forward_pops_from_forward_and_pushes_to_back() {
        let mut h = History::new();
        h.navigate("a".into(), "b".into()); // back=[a]
        h.back("b".into()); // forward=[b], back=[]
        let next = h.forward("a".into()).unwrap(); // back=[a], forward=[]
        assert_eq!(next, "b");
        assert_eq!(h.forward("b".into()), None);
    }

    #[test]
    fn navigate_after_back_clears_forward() {
        let mut h = History::new();
        h.navigate("a".into(), "b".into());
        h.back("b".into()); // forward=[b]
        h.navigate("a".into(), "c".into()); // clears forward
        assert_eq!(h.forward("c".into()), None);
    }

    #[test]
    fn multiple_navigations_build_deep_back_stack() {
        let mut h = History::new();
        h.navigate("a".into(), "b".into());
        h.navigate("b".into(), "c".into());
        h.navigate("c".into(), "d".into());
        assert_eq!(h.back("d".into()), Some("c".into()));
        assert_eq!(h.back("c".into()), Some("b".into()));
        assert_eq!(h.back("b".into()), Some("a".into()));
        assert_eq!(h.back("a".into()), None);
    }
}
