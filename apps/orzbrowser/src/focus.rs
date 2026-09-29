//! Folds the focus changes both webviews reported in one loop pass.

use ratatui_orzma::FocusChange;

/// One of orzbrowser's two webviews.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Target {
    /// The remote page.
    Page,
    /// The chrome at the top.
    Chrome,
}

/// The focus changes one webview reported in one loop pass.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct HandleFocus {
    /// Whether any change reported focus.
    pub(crate) saw_true: bool,
    /// The last change, or `None` when none arrived.
    pub(crate) last: Option<bool>,
}

/// Both webviews' focus changes from one loop pass.
///
/// The page's changes must be read before the chrome's. Both can then end on
/// a gain only when focus moved from the page to the chrome between the two
/// reads, so the chrome's gain is the later one.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct FocusDrain {
    /// The page's changes.
    pub(crate) page: HandleFocus,
    /// The chrome's changes.
    pub(crate) chrome: HandleFocus,
}

impl HandleFocus {
    /// Folds `changes`, oldest first.
    pub fn fold(changes: impl IntoIterator<Item = bool>) -> Self {
        changes
            .into_iter()
            .fold(Self::default(), |acc, focused| Self {
                saw_true: acc.saw_true || focused,
                last: Some(focused),
            })
    }
}

impl FocusDrain {
    /// Folds the changes each webview reported this pass, oldest first.
    pub fn from_changes(page: Vec<FocusChange>, chrome: Vec<FocusChange>) -> Self {
        Self {
            page: HandleFocus::fold(page.iter().map(|change| change.focused)),
            chrome: HandleFocus::fold(chrome.iter().map(|change| change.focused)),
        }
    }

    /// Whether neither webview reported a change.
    pub fn is_empty(&self) -> bool {
        self.page.last.is_none() && self.chrome.last.is_none()
    }

    /// The changes `target` reported.
    pub fn of(&self, target: Target) -> HandleFocus {
        match target {
            Target::Page => self.page,
            Target::Chrome => self.chrome,
        }
    }

    /// Whether either webview reported gaining focus.
    pub fn saw_any_true(&self) -> bool {
        self.page.saw_true || self.chrome.saw_true
    }

    /// Returns `holder` updated by this pass: a webview whose last change is a
    /// gain holds focus, and one whose last change is a loss stops holding it.
    /// When both end on a gain, the chrome holds focus.
    pub fn holder_after(&self, holder: Option<Target>) -> Option<Target> {
        let mut next = holder;
        for target in [Target::Page, Target::Chrome] {
            if self.of(target).last == Some(false) && next == Some(target) {
                next = None;
            }
        }
        for target in [Target::Page, Target::Chrome] {
            if self.of(target).last == Some(true) {
                next = Some(target);
            }
        }
        next
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drain(page: &[bool], chrome: &[bool]) -> FocusDrain {
        FocusDrain {
            page: HandleFocus::fold(page.iter().copied()),
            chrome: HandleFocus::fold(chrome.iter().copied()),
        }
    }

    /// Asserts that folding keeps whether focus was seen and the last value.
    ///
    /// Case: a webview gains and loses focus within one loop pass, loses it
    /// only, or reports nothing.
    #[test]
    fn folding_keeps_the_last_value_and_any_gain() {
        assert_eq!(
            HandleFocus::fold([true, false]),
            HandleFocus {
                saw_true: true,
                last: Some(false)
            }
        );
        assert_eq!(
            HandleFocus::fold([false]),
            HandleFocus {
                saw_true: false,
                last: Some(false)
            }
        );
        assert_eq!(HandleFocus::fold([]), HandleFocus::default());
        assert!(drain(&[], &[]).is_empty());
        assert!(!drain(&[false], &[]).is_empty());
    }

    /// Asserts that the holder follows the last gain, and that a loss clears
    /// only the webview that held focus.
    ///
    /// Case: the user clicks the page, clicks the chrome, then the chrome's
    /// focus is released; a stale loss arrives for a webview that no longer
    /// holds focus; focus moves from the page to the chrome between the two
    /// reads of one pass.
    #[test]
    fn the_holder_follows_the_last_gain() {
        assert_eq!(drain(&[true], &[]).holder_after(None), Some(Target::Page));
        assert_eq!(
            drain(&[false], &[true]).holder_after(Some(Target::Page)),
            Some(Target::Chrome)
        );
        assert_eq!(drain(&[], &[false]).holder_after(Some(Target::Chrome)), None);
        assert_eq!(
            drain(&[], &[false]).holder_after(Some(Target::Page)),
            Some(Target::Page)
        );
        assert_eq!(
            drain(&[true], &[false]).holder_after(None),
            Some(Target::Page)
        );
        assert_eq!(
            drain(&[true], &[true]).holder_after(Some(Target::Page)),
            Some(Target::Chrome)
        );
    }

    /// Asserts that a gain on either webview is reported.
    ///
    /// Case: the user clicks the chrome or the page while the help is open.
    #[test]
    fn any_gain_is_reported() {
        assert!(drain(&[], &[true, false]).saw_any_true());
        assert!(drain(&[true], &[]).saw_any_true());
        assert!(!drain(&[false], &[false]).saw_any_true());
    }
}
