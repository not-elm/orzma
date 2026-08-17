use orzma_vt::screen::viewport::DisplayOffset;

#[test]
fn display_offset_is_owned_by_the_viewport_module() {
    assert_eq!(DisplayOffset::default(), DisplayOffset(0));
}
