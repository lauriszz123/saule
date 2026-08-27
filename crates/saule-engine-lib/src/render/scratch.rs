//! Reusable working buffers.
//!
//! Every shape used to allocate on its way to the rasterizer: a device-space
//! copy of the path, a `Vec<Vec<Point>>` of stroke outlines, and — inside the
//! filler — a coverage buffer as wide as the shape's bounding box, zeroed on
//! every call. A UI painting a few hundred shapes across a wide window spent
//! megabytes per frame on allocate-and-zero alone.
//!
//! Nothing here changes what is drawn. The buffers live on the
//! [`Renderer`](super::Renderer) and are cleared rather than freed, so a steady
//! frame settles into allocating nothing at all.

use crate::font::WrapBuf;
use crate::geom::{PathSet, Point, StrokeScratch};
use crate::raster::FillScratch;

/// The renderer's working memory.
#[derive(Default)]
pub struct Scratch {
    /// The shape currently being drawn, in local coordinates. The path
    /// builders write here rather than returning a fresh `Vec` per shape.
    pub(crate) local: Vec<Point>,
    /// A path transformed into device space, before it is filled or stroked.
    pub(crate) device: Vec<Point>,
    /// Outlines handed to the filler — one per fill, several per stroke.
    pub(crate) paths: PathSet,
    /// The stroke expander's deduplicated points and per-segment directions.
    pub(crate) stroke: StrokeScratch,
    /// The filler's own edge list and coverage row.
    pub(crate) fill: FillScratch,
    /// Laid-out glyph positions for one line of text.
    pub(crate) glyphs: Vec<(char, f64)>,
    /// Wrapped lines produced by `printf`.
    pub(crate) wrap: WrapBuf,
}
