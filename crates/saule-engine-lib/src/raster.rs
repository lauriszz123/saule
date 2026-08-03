//! Software rasterizer: pixel surfaces, blend modes, an antialiased polygon
//! filler, and the two blitters (alpha masks for glyphs, ARGB surfaces for
//! canvases).
//!
//! Everything here works in **device coordinates** — [`crate::state`] applies
//! the current transform before calling in. Coverage is computed by sampling
//! [`SUBSAMPLES`] sub-scanlines per pixel row and accumulating exact horizontal
//! span overlap, which is what gives shape edges and text their smooth edges
//! without a GPU.

mod blit;
mod fill;
mod pixel;
#[cfg(test)]
mod tests;

pub(crate) use blit::*;
pub(crate) use fill::*;
pub(crate) use pixel::*;
