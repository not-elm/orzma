//! Tests for the [`Vt`] trait implementation on [`OldOrzmaVt`].

use super::*;

/// Asserts that interpreting an empty chunk returns
/// [`VtUpdate::default`] with nothing drained into it.
///
/// The Vt contract promises the default for an empty chunk; draining
/// the backend anyway would attribute signals or replies a previous
/// chunk buffered to a chunk that produced nothing.
///
/// Case: the owner forwards a zero-length PTY read to the VT
/// unfiltered while earlier output is still buffered backend-side.
#[test]
fn an_empty_chunk_interprets_to_the_default_update() {
    let mut vt = clean_vt();
    let update = Vt::interpret(&mut vt, b"");
    assert!(update.verdict.is_none());
    assert!(update.signals.is_empty());
    assert!(update.replies.is_empty());
}
