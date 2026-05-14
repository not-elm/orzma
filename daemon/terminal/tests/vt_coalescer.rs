//! Integration tests for the per-bridge frame coalescer.
//!
//! These tests drive `TerminalService` with real PTYs and assert wire-frame
//! invariants under timing pressure. Real PTYs are used (not mocks) so the
//! tests exercise the same code path production hits.

#[tokio::test]
async fn placeholder_until_bridge_rewrite_lands() {
    // NOTE: real tests are added in Tasks 5 and 7 once the bridge is rewritten.
}
