//! The in-memory Markdown document plus path resolution and change detection.

use crate::outline::{self, Heading};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// A loaded Markdown document and its derived outline.
#[derive(Debug, Clone)]
pub(crate) struct Document {
    /// Raw Markdown source.
    pub(crate) text: String,
    /// Absolute parent directory of the source file.
    pub(crate) base_dir: PathBuf,
    /// Headings parsed from `text`, in document order.
    pub(crate) outline: Vec<Heading>,
}

/// A cheap change fingerprint: file length plus mtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Fingerprint {
    len: u64,
    mtime: Option<SystemTime>,
}

impl Document {
    /// Builds a document from source text and the file's parent directory,
    /// parsing the outline from `text`.
    fn from_source(text: String, base_dir: PathBuf) -> Self {
        let outline = outline::parse(&text);
        Self {
            text,
            base_dir,
            outline,
        }
    }
}

/// Resolves a user-supplied path to an absolute, canonical regular-file path.
///
/// # Errors
/// Returns an error if the path does not exist or is not a regular file.
pub(crate) fn resolve_path(arg: &str) -> io::Result<PathBuf> {
    require_regular_file(arg)
}

/// Reads and parses the Markdown file at `path` into a [`Document`].
pub(crate) fn load(path: &Path) -> io::Result<Document> {
    let text = fs::read_to_string(path)?;
    let base_dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
    Ok(Document::from_source(text, base_dir))
}

/// Resolves a link `target` (relative or absolute) against `base_dir` to an
/// absolute, canonical path, requiring it to be an existing regular file.
///
/// # Errors
/// Returns an error if the path does not exist or is not a regular file.
pub(crate) fn resolve_link(base_dir: &Path, target: &str) -> io::Result<PathBuf> {
    require_regular_file(base_dir.join(target))
}

fn require_regular_file(path: impl AsRef<Path>) -> io::Result<PathBuf> {
    let path = fs::canonicalize(path)?;
    if !path.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a regular file",
        ));
    }
    Ok(path)
}

/// Whether `path` has a Markdown extension (`.md` or `.markdown`, ASCII
/// case-insensitive).
pub(crate) fn is_markdown(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("md") || ext.eq_ignore_ascii_case("markdown"))
}

/// Reads the change fingerprint (length + mtime) for `path`.
pub(crate) fn fingerprint(path: &Path) -> io::Result<Fingerprint> {
    let meta = fs::metadata(path)?;
    Ok(Fingerprint {
        len: meta.len(),
        mtime: meta.modified().ok(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_temp(name: &str, body: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(name);
        let mut f = fs::File::create(&path).unwrap();
        f.write_all(body.as_bytes()).unwrap();
        (dir, path)
    }

    #[test]
    fn load_reads_text_outline_and_base_dir() {
        let (_dir, path) = write_temp("doc.md", "# A\n\ntext\n## B\n");
        let doc = load(&path).unwrap();
        assert_eq!(doc.text, "# A\n\ntext\n## B\n");
        assert_eq!(doc.outline.len(), 2);
        assert_eq!(doc.base_dir, path.parent().unwrap());
    }

    #[test]
    fn resolve_path_rejects_directories() {
        let dir = tempfile::tempdir().unwrap();
        assert!(resolve_path(dir.path().to_str().unwrap()).is_err());
    }

    #[test]
    fn resolve_path_rejects_missing() {
        assert!(resolve_path("/no/such/file/ozmd-xyz.md").is_err());
    }

    #[test]
    fn fingerprint_changes_with_content_length() {
        let (_dir, path) = write_temp("doc.md", "short");
        let fp1 = fingerprint(&path).unwrap();
        fs::write(&path, "a much longer body than before").unwrap();
        let fp2 = fingerprint(&path).unwrap();
        assert_ne!(fp1, fp2);
    }

    #[test]
    fn resolve_link_resolves_relative_against_base_dir() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("child.md");
        fs::write(&target, "x").unwrap();
        let got = resolve_link(dir.path(), "child.md").unwrap();
        assert_eq!(got, fs::canonicalize(&target).unwrap());
    }

    #[test]
    fn resolve_link_handles_parent_and_absolute() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("sub");
        fs::create_dir(&sub).unwrap();
        let sibling = dir.path().join("sib.md");
        fs::write(&sibling, "x").unwrap();
        let from_parent = resolve_link(&sub, "../sib.md").unwrap();
        assert_eq!(from_parent, fs::canonicalize(&sibling).unwrap());
        let abs = sibling.to_str().unwrap();
        assert_eq!(
            resolve_link(&sub, abs).unwrap(),
            fs::canonicalize(&sibling).unwrap()
        );
    }

    #[test]
    fn resolve_link_rejects_missing_and_directories() {
        let dir = tempfile::tempdir().unwrap();
        assert!(resolve_link(dir.path(), "nope.md").is_err());
        assert!(resolve_link(dir.path(), ".").is_err());
    }

    #[test]
    fn is_markdown_matches_extensions_case_insensitively() {
        assert!(is_markdown(Path::new("a.md")));
        assert!(is_markdown(Path::new("a.MARKDOWN")));
        assert!(is_markdown(Path::new("/x/y.Md")));
        assert!(!is_markdown(Path::new("a.txt")));
        assert!(!is_markdown(Path::new("README")));
    }
}
