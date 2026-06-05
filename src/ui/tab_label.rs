//! Tab-title label formatting. `tab_label` renders a terminal surface's live
//! `Cwd` (home-abbreviated, front-truncated) and a browser/extension surface's
//! live `WebTitle` (sanitized, back-truncated), blank until a title arrives;
//! `LabelCtx` carries the per-rebuild inputs.

use crate::ui::web_title::WebTitle;
use bevy_terminal::sanitize_title;
use ozmux_multiplexer::{Cwd, SurfaceKind};
use std::path::{Path, PathBuf};

/// Placeholder shown for a terminal surface whose `Cwd` is not yet known.
const TERMINAL_PLACEHOLDER: &str = "";
/// Placeholder shown for a browser/extension surface with no webview title yet.
const WEB_PLACEHOLDER: &str = "";

/// Per-refresh inputs for `tab_label`, built once per `refresh_pane_tabs` run
/// and threaded through the per-pane tab rebuild.
pub(crate) struct LabelCtx {
    pub(crate) home: Option<PathBuf>,
    pub(crate) max_chars: usize,
}

/// Returns the tab-title string for a surface. Terminal surfaces render their
/// home-abbreviated, front-truncated `Cwd` (`TERMINAL_PLACEHOLDER` when unknown).
/// Browser/extension surfaces render their sanitized, back-truncated `WebTitle`
/// (`WEB_PLACEHOLDER` when absent or empty).
pub(crate) fn tab_label(
    kind: &SurfaceKind,
    cwd: Option<&Cwd>,
    web_title: Option<&WebTitle>,
    home: Option<&Path>,
    max_chars: usize,
) -> String {
    if !matches!(kind, SurfaceKind::Terminal) {
        let raw = web_title.map(|t| t.0.as_str()).unwrap_or("");
        if raw.is_empty() {
            return WEB_PLACEHOLDER.to_string();
        }
        let sanitized = sanitize_title(raw);
        if sanitized.is_empty() {
            return WEB_PLACEHOLDER.to_string();
        }
        return back_truncate(&sanitized, max_chars);
    }
    let Some(Cwd(path)) = cwd else {
        return TERMINAL_PLACEHOLDER.to_string();
    };
    let abbreviated = abbreviate_home(path, home);
    let sanitized = sanitize_title(&abbreviated);
    front_truncate(&sanitized, max_chars)
}

/// Back-truncates `s` to at most `max_chars` chars, keeping the front and
/// appending `…`. Used for web page titles (leading text is most identifying) —
/// the mirror image of the path-oriented `front_truncate`.
fn back_truncate(s: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let head: String = s.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{head}…")
}

/// Replaces a leading `home` prefix with `~` (component-aware), else returns
/// the lossy path string.
fn abbreviate_home(path: &Path, home: Option<&Path>) -> String {
    if let Some(home) = home
        && let Ok(rest) = path.strip_prefix(home)
    {
        let rest = rest.to_string_lossy();
        return if rest.is_empty() {
            "~".to_string()
        } else {
            format!("~/{rest}")
        };
    }
    path.to_string_lossy().into_owned()
}

/// Front-truncates `s` to at most `max_chars` chars, dropping leading path
/// segments and prepending `…/`. A single over-long trailing segment is
/// hard-cut with a leading `…`.
fn front_truncate(s: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let budget = max_chars.saturating_sub(2);
    // NOTE: empty segments must be filtered — an OSC 7 trailing slash makes
    // `split('/')` yield a 0-length segment whose `extra` is always 0, so it
    // would slip into `kept` and leak a stray separator (or overflow a tiny
    // budget).
    let segments: Vec<&str> = s.split('/').filter(|seg| !seg.is_empty()).collect();
    let mut kept: Vec<&str> = Vec::new();
    let mut used = 0usize;
    for seg in segments.iter().rev() {
        let seg_len = seg.chars().count();
        let extra = if kept.is_empty() {
            seg_len
        } else {
            seg_len + 1
        };
        if used + extra > budget {
            break;
        }
        used += extra;
        kept.push(seg);
    }
    if kept.is_empty() {
        let last = segments.last().copied().unwrap_or("");
        let chars: Vec<char> = last.chars().collect();
        let take = max_chars.saturating_sub(1);
        let start = chars.len().saturating_sub(take);
        let tail: String = chars[start..].iter().collect();
        return format!("…{tail}");
    }
    kept.reverse();
    format!("…/{}", kept.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAX: usize = 28;

    fn term() -> SurfaceKind {
        SurfaceKind::Terminal
    }

    #[test]
    fn terminal_under_home_abbreviates() {
        let cwd = Cwd(PathBuf::from("/home/u/proj"));
        let out = tab_label(&term(), Some(&cwd), None, Some(Path::new("/home/u")), MAX);
        assert_eq!(out, "~/proj");
    }

    #[test]
    fn terminal_at_home_is_tilde() {
        let cwd = Cwd(PathBuf::from("/home/u"));
        let out = tab_label(&term(), Some(&cwd), None, Some(Path::new("/home/u")), MAX);
        assert_eq!(out, "~");
    }

    #[test]
    fn terminal_outside_home_is_absolute() {
        let cwd = Cwd(PathBuf::from("/etc/nginx"));
        let out = tab_label(&term(), Some(&cwd), None, Some(Path::new("/home/u")), MAX);
        assert_eq!(out, "/etc/nginx");
    }

    #[test]
    fn home_prefix_is_component_aware() {
        let cwd = Cwd(PathBuf::from("/home/u2/x"));
        let out = tab_label(&term(), Some(&cwd), None, Some(Path::new("/home/u")), MAX);
        assert_eq!(out, "/home/u2/x");
    }

    #[test]
    fn no_home_keeps_absolute() {
        let cwd = Cwd(PathBuf::from("/home/u/proj"));
        let out = tab_label(&term(), Some(&cwd), None, None, MAX);
        assert_eq!(out, "/home/u/proj");
    }

    #[test]
    fn long_path_front_ellipsizes() {
        let cwd = Cwd(PathBuf::from("/home/u/workspace/ozmux/wt/tab-title"));
        let out = tab_label(&term(), Some(&cwd), None, Some(Path::new("/home/u")), MAX);
        assert!(out.starts_with("…/"), "got {out}");
        assert!(out.ends_with("tab-title"), "got {out}");
        assert!(
            out.chars().count() <= MAX,
            "got {out} ({} chars)",
            out.chars().count()
        );
    }

    #[test]
    fn single_long_segment_hard_cuts() {
        let seg = "a".repeat(100);
        let cwd = Cwd(PathBuf::from(format!("/{seg}")));
        let out = tab_label(&term(), Some(&cwd), None, None, MAX);
        assert!(out.starts_with('…'), "got {out}");
        assert_eq!(out.chars().count(), MAX);
    }

    fn browser() -> SurfaceKind {
        use ozmux_multiplexer::BrowserProfile;
        SurfaceKind::Browser {
            initial_url: None,
            profile: BrowserProfile::default(),
        }
    }

    fn ext() -> SurfaceKind {
        SurfaceKind::Extension {
            entry: PathBuf::from("/tmp/ext"),
        }
    }

    #[test]
    fn browser_renders_web_title() {
        let wt = WebTitle("GitHub".into());
        assert_eq!(tab_label(&browser(), None, Some(&wt), None, MAX), "GitHub");
    }

    #[test]
    fn extension_renders_web_title() {
        let wt = WebTitle("memo".into());
        assert_eq!(tab_label(&ext(), None, Some(&wt), None, MAX), "memo");
    }

    #[test]
    fn browser_blank_without_title() {
        assert_eq!(tab_label(&browser(), None, None, None, MAX), "");
    }

    #[test]
    fn browser_blank_on_empty_title() {
        let wt = WebTitle(String::new());
        assert_eq!(tab_label(&browser(), None, Some(&wt), None, MAX), "");
    }

    #[test]
    fn web_title_back_truncates() {
        let wt = WebTitle("A very long page title that exceeds the budget".into());
        let out = tab_label(&browser(), None, Some(&wt), None, MAX);
        assert!(out.starts_with("A very"), "got {out}");
        assert!(out.ends_with('…'), "got {out}");
        assert!(
            out.chars().count() <= MAX,
            "got {out} ({} chars)",
            out.chars().count()
        );
    }

    #[test]
    fn web_title_truncate_boundary() {
        let wt = WebTitle("hello".into());
        assert_eq!(tab_label(&browser(), None, Some(&wt), None, 1), "…");
        assert_eq!(tab_label(&browser(), None, Some(&wt), None, 0), "");
    }

    #[test]
    fn web_title_control_chars_stripped() {
        let wt = WebTitle("a\u{7}b".into());
        assert_eq!(tab_label(&ext(), None, Some(&wt), None, MAX), "ab");
    }

    #[test]
    fn control_chars_stripped() {
        let cwd = Cwd(PathBuf::from("/tmp/a\u{7}b"));
        let out = tab_label(&term(), Some(&cwd), None, None, MAX);
        assert_eq!(out, "/tmp/ab");
    }

    #[test]
    fn tiny_max_chars_never_exceeds_budget() {
        let cwd = Cwd(PathBuf::from("/home/u/workspace"));
        let out0 = tab_label(&term(), Some(&cwd), None, Some(Path::new("/home/u")), 0);
        assert_eq!(out0.chars().count(), 0, "got {out0:?}");
        let out1 = tab_label(&term(), Some(&cwd), None, Some(Path::new("/home/u")), 1);
        assert_eq!(out1.chars().count(), 1, "got {out1:?}");
    }

    #[test]
    fn multibyte_path_abbreviates() {
        let cwd = Cwd(PathBuf::from("/home/u/プロジェクト"));
        let out = tab_label(&term(), Some(&cwd), None, Some(Path::new("/home/u")), MAX);
        assert_eq!(out, "~/プロジェクト");
    }

    #[test]
    fn trailing_slash_does_not_leak_or_overflow() {
        let cwd = Cwd(PathBuf::from("/home/u/workspace/ozmux/wt/tab-title/"));
        let out = tab_label(&term(), Some(&cwd), None, Some(Path::new("/home/u")), MAX);
        assert!(
            out.starts_with("…/") && out.ends_with("tab-title"),
            "got {out}"
        );
        assert!(!out.ends_with('/'), "trailing slash leaked: {out}");
        assert!(out.chars().count() <= MAX, "got {out}");
        let tiny = tab_label(&term(), Some(&cwd), None, None, 1);
        assert_eq!(tiny.chars().count(), 1, "got {tiny:?}");
    }
}
