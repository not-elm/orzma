//! The control tokens the host issues: which pane each token binds, and
//! which panes are live.

use crate::error::WebviewHostResult;
use crate::host::PaneKey;
use crate::host::mint::random_base32;
use std::collections::{HashMap, HashSet};

/// The token issued to each live pane, and the set of live panes.
pub(crate) struct Tokens<P> {
    live: HashSet<P>,
    by_token: HashMap<String, P>,
}

impl<P: PaneKey> Tokens<P> {
    /// No live pane and no token.
    pub fn new() -> Self {
        Self {
            live: HashSet::new(),
            by_token: HashMap::new(),
        }
    }

    /// Marks `pane` live.
    pub fn mark_live(&mut self, pane: P) {
        self.live.insert(pane);
    }

    /// Mints a token spelled `orzma:<random base32>` and binds it to `pane`.
    pub fn issue(&mut self, pane: P) -> WebviewHostResult<String> {
        let token = format!("orzma:{}", random_base32()?);
        self.by_token.insert(token.clone(), pane);
        Ok(token)
    }

    /// The live pane `token` is bound to, or `None`.
    pub fn resolve(&self, token: &str) -> Option<P> {
        self.by_token
            .get(token)
            .copied()
            .filter(|pane| self.live.contains(pane))
    }

    /// Whether `pane` is live.
    pub fn is_live(&self, pane: P) -> bool {
        self.live.contains(&pane)
    }

    /// Forgets `pane` and every token bound to it.
    pub fn forget(&mut self, pane: P) {
        self.live.remove(&pane);
        self.by_token.retain(|_, bound| *bound != pane);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Asserts that an issued token resolves to its live pane, and stops
    /// resolving once the pane is forgotten.
    ///
    /// Case: a pane's shell connects with its token, and after the pane
    /// closes a leftover process retries with the same token.
    #[test]
    fn a_token_resolves_only_while_its_pane_lives() {
        let mut tokens = Tokens::new();
        tokens.mark_live(1_u32);
        let token = tokens.issue(1).expect("a token mints");
        assert!(token.starts_with("orzma:"));
        assert_eq!(tokens.resolve(&token), Some(1));
        assert_eq!(tokens.resolve("orzma:unknown"), None);
        tokens.forget(1);
        assert_eq!(tokens.resolve(&token), None);
        assert!(!tokens.is_live(1));
    }

    /// Asserts that two panes get different tokens.
    ///
    /// Case: the user splits a pane and both shells connect.
    #[test]
    fn two_panes_get_different_tokens() {
        let mut tokens = Tokens::new();
        tokens.mark_live(1_u32);
        tokens.mark_live(2);
        let first = tokens.issue(1).expect("mints");
        let second = tokens.issue(2).expect("mints");
        assert_ne!(first, second);
        assert_eq!(tokens.resolve(&second), Some(2));
    }
}
