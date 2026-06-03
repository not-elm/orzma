//! Tab-title label formatting. `tab_label` renders a terminal surface's live
//! `Cwd` (home-abbreviated, front-ellipsized) and falls back to the surface
//! `Name` for non-terminal kinds; `LabelCtx` carries the per-rebuild inputs.

use bevy_terminal::sanitize_title;
use ozmux_multiplexer::{Cwd, SurfaceKind};
use std::path::{Path, PathBuf};

/// Placeholder shown for a terminal surface whose `Cwd` is not yet known.
const TERMINAL_PLACEHOLDER: &str = "terminal";

/// Per-rebuild inputs for `tab_label`, built once in `rebuild_session_ui` and
/// threaded through the cell-tree builder.
pub(crate) struct LabelCtx {
    pub(crate) home: Option<PathBuf>,
    pub(crate) max_chars: usize,
}

/// Returns the tab-title string for a surface. Terminal surfaces render their
/// home-abbreviated, front-ellipsized `Cwd`; an unknown `Cwd` yields
/// `TERMINAL_PLACEHOLDER`. Non-terminal kinds return `name` unchanged.
pub(crate) fn tab_label(
    kind: &SurfaceKind,
    cwd: Option<&Cwd>,
    name: &str,
    home: Option<&Path>,
    max_chars: usize,
) -> String {
    if !matches!(kind, SurfaceKind::Terminal) {
        return name.to_string();
    }
    let Some(Cwd(path)) = cwd else {
        return TERMINAL_PLACEHOLDER.to_string();
    };
    let abbreviated = abbreviate_home(path, home);
    let sanitized = sanitize_title(&abbreviated);
    front_truncate(&sanitized, max_chars)
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
    let segments: Vec<&str> = s.split('/').collect();
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
        let out = tab_label(&term(), Some(&cwd), "x", Some(Path::new("/home/u")), MAX);
        assert_eq!(out, "~/proj");
    }

    #[test]
    fn terminal_at_home_is_tilde() {
        let cwd = Cwd(PathBuf::from("/home/u"));
        let out = tab_label(&term(), Some(&cwd), "x", Some(Path::new("/home/u")), MAX);
        assert_eq!(out, "~");
    }

    #[test]
    fn terminal_outside_home_is_absolute() {
        let cwd = Cwd(PathBuf::from("/etc/nginx"));
        let out = tab_label(&term(), Some(&cwd), "x", Some(Path::new("/home/u")), MAX);
        assert_eq!(out, "/etc/nginx");
    }

    #[test]
    fn home_prefix_is_component_aware() {
        let cwd = Cwd(PathBuf::from("/home/u2/x"));
        let out = tab_label(&term(), Some(&cwd), "x", Some(Path::new("/home/u")), MAX);
        assert_eq!(out, "/home/u2/x");
    }

    #[test]
    fn no_home_keeps_absolute() {
        let cwd = Cwd(PathBuf::from("/home/u/proj"));
        let out = tab_label(&term(), Some(&cwd), "x", None, MAX);
        assert_eq!(out, "/home/u/proj");
    }

    #[test]
    fn long_path_front_ellipsizes() {
        let cwd = Cwd(PathBuf::from("/home/u/workspace/ozmux/wt/tab-title"));
        let out = tab_label(&term(), Some(&cwd), "x", Some(Path::new("/home/u")), MAX);
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
        let out = tab_label(&term(), Some(&cwd), "x", None, MAX);
        assert!(out.starts_with('…'), "got {out}");
        assert_eq!(out.chars().count(), MAX);
    }

    #[test]
    fn no_cwd_is_placeholder() {
        let out = tab_label(&term(), None, "ignored", None, MAX);
        assert_eq!(out, "terminal");
    }

    #[test]
    fn extension_returns_name() {
        let kind = SurfaceKind::Extension {
            entry: PathBuf::from("/tmp/ext"),
        };
        let out = tab_label(&kind, None, "memo", None, MAX);
        assert_eq!(out, "memo");
    }

    #[test]
    fn browser_returns_name() {
        use ozmux_multiplexer::BrowserProfile;
        let kind = SurfaceKind::Browser {
            initial_url: None,
            profile: BrowserProfile::default(),
        };
        let cwd = Cwd(PathBuf::from("/home/u/proj"));
        let out = tab_label(
            &kind,
            Some(&cwd),
            "my-browser",
            Some(Path::new("/home/u")),
            MAX,
        );
        assert_eq!(out, "my-browser");
    }

    #[test]
    fn control_chars_stripped() {
        let cwd = Cwd(PathBuf::from("/tmp/a\u{7}b"));
        let out = tab_label(&term(), Some(&cwd), "x", None, MAX);
        assert_eq!(out, "/tmp/ab");
    }

    #[test]
    fn tiny_max_chars_never_exceeds_budget() {
        let cwd = Cwd(PathBuf::from("/home/u/workspace"));
        let out0 = tab_label(&term(), Some(&cwd), "x", Some(Path::new("/home/u")), 0);
        assert_eq!(out0.chars().count(), 0, "got {out0:?}");
        let out1 = tab_label(&term(), Some(&cwd), "x", Some(Path::new("/home/u")), 1);
        assert_eq!(out1.chars().count(), 1, "got {out1:?}");
    }

    #[test]
    fn multibyte_path_abbreviates() {
        let cwd = Cwd(PathBuf::from("/home/u/プロジェクト"));
        let out = tab_label(&term(), Some(&cwd), "x", Some(Path::new("/home/u")), MAX);
        assert_eq!(out, "~/プロジェクト");
    }
}
