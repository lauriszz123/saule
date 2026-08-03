use super::*;
use crate::geom::{LineJoin, Transform};
use crate::raster::BlendMode;

#[test]
fn default_state_matches_love_defaults() {
    let s = GState::default();
    assert_eq!(s.color, [1.0, 1.0, 1.0, 1.0]);
    assert_eq!(s.line_width, 1.0);
    assert_eq!(s.line_join, LineJoin::Miter);
    assert_eq!(s.blend, BlendMode::Alpha);
    assert!(s.smooth);
    assert!(s.scissor.is_none());
    assert_eq!(s.transform, Transform::IDENTITY);
    assert_eq!(s.font, 0);
}
