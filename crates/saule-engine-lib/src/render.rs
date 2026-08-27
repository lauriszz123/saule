//! The renderer: render targets, the graphics state machine, the resource
//! registries, and every `Graphics.*` operation that does not touch the OS.
//!
//! This module deliberately knows nothing about windows, input, or frame
//! pacing. Everything here operates on plain [`Surface`] buffers, which is
//! what makes the whole drawing pipeline testable without a display —
//! [`Renderer::headless`] builds one against an offscreen screen surface and
//! the tests in `render/tests.rs` assert on the resulting pixels.
//!
//! [`crate::state::Engine`] owns a `Renderer` alongside the OS window and
//! forwards the drawing calls to it.

mod canvas;
mod draw;
mod scratch;
mod text;

#[cfg(test)]
mod tests;

pub(crate) use canvas::*;
pub(crate) use draw::*;
pub(crate) use scratch::*;

use std::sync::atomic::{AtomicU32, Ordering};

use crate::font::FontRes;
use crate::raster::Surface;

/// Everything `Graphics.*` draws with.
pub struct Renderer {
    /// The window's framebuffer. minifb reads it as `0RGB` and ignores alpha.
    pub(crate) screen: Surface,
    /// Canvas and image registry. Handle `0` is the screen.
    pub(crate) canvases: Vec<Slot<Surface>>,
    /// Indices of released canvas slots, reused before the vector grows.
    pub(crate) free_canvases: Vec<usize>,
    /// Bound render target: `None` is the screen, `Some(i)` a canvas index.
    pub(crate) target: Option<usize>,
    /// Font registry. Slot 0 is the system default, loaded on first use, and
    /// is the only slot that is never allocated or released.
    pub(crate) fonts: Vec<Slot<FontRes>>,
    pub(crate) free_fonts: Vec<usize>,
    pub(crate) background: [f64; 4],
    pub(crate) st: GState,
    pub(crate) stack: Vec<Saved>,
    /// Bilinear rather than nearest sampling for transformed blits.
    pub(crate) linear_filter: bool,
    /// Reusable working buffers, so a frame of drawing allocates nothing.
    pub(crate) scratch: Scratch,
}

impl Renderer {
    /// A renderer targeting a `width` × `height` opaque screen surface.
    pub fn new(width: usize, height: usize) -> Self {
        Renderer {
            screen: Surface::opaque(width, height),
            canvases: Vec::new(),
            free_canvases: Vec::new(),
            target: None,
            // Slot 0 is the default face: reserved, lazily filled, never freed.
            fonts: vec![Slot::reserved()],
            free_fonts: Vec::new(),
            background: [0.0, 0.0, 0.0, 1.0],
            st: GState::default(),
            stack: Vec::new(),
            linear_filter: true,
            scratch: Scratch::default(),
        }
    }

    /// A renderer with no window behind it, for tests and offscreen work.
    #[cfg(test)]
    pub fn headless(width: usize, height: usize) -> Self {
        Renderer::new(width, height)
    }

    /// Replace the screen surface — what a window resize does.
    ///
    /// A scissor set against the old size could sit entirely outside the new
    /// one, which would silently blank every following frame, so the clip is
    /// dropped at the same moment.
    pub(crate) fn resize_screen(&mut self, width: usize, height: usize) {
        self.screen = Surface::opaque(width, height);
        self.st.scissor = None;
    }
}

// ---------------------------------------------------------------------------
// Resource handles
// ---------------------------------------------------------------------------

/// Process-global allocation counter. Every resource slot records the tag it
/// was allocated with, and a handle carries that tag beside the slot index.
///
/// This is what makes a stale handle an *error* rather than a silent alias:
/// a released slot bumps to a new tag when it is reused, and because the
/// counter is global rather than per-window, a handle left over from a
/// previous `Window.create` can never match a slot in the new one either.
static NEXT_TAG: AtomicU32 = AtomicU32::new(1);

fn next_tag() -> u32 {
    // Wrapping is theoretical (2^32 allocations), and wrapping back to 0 would
    // collide with the "free" marker, so skip it if it ever happens.
    match NEXT_TAG.fetch_add(1, Ordering::Relaxed) {
        0 => NEXT_TAG.fetch_add(1, Ordering::Relaxed),
        tag => tag,
    }
}

/// One registry slot.
///
/// `tag == 0` means the slot is free. A live slot whose value is `None` is on
/// loan to its own draw call (see [`Renderer::draw_canvas`]).
pub(crate) struct Slot<T> {
    tag: u32,
    value: Option<T>,
}

impl<T> Slot<T> {
    /// A live slot with nothing in it yet — the default font's slot.
    fn reserved() -> Self {
        Slot {
            tag: 0,
            value: None,
        }
    }

    fn free() -> Self {
        Slot {
            tag: 0,
            value: None,
        }
    }

    fn is_free(&self) -> bool {
        self.tag == 0
    }
}

/// Pack a slot tag and index into the integer handle scripts hold.
///
/// The index is stored one-based so a valid handle is never `0`, which is
/// reserved for "the screen" (canvases) and "the default face" (fonts).
fn pack_handle(tag: u32, idx: usize) -> i64 {
    ((tag as i64) << 32) | (idx as i64 + 1)
}

/// Split a handle back into `(tag, index)`. `None` for anything malformed.
fn unpack_handle(handle: i64) -> Option<(u32, usize)> {
    if handle <= 0 {
        return None;
    }
    let idx = (handle & 0xFFFF_FFFF) as usize;
    if idx == 0 {
        return None;
    }
    Some(((handle >> 32) as u32, idx - 1))
}

/// Resolve `handle` against a registry, naming `func` in any error.
fn resolve<T>(slots: &[Slot<T>], handle: i64, func: &str, what: &str) -> Result<usize, String> {
    let Some((tag, idx)) = unpack_handle(handle) else {
        return Err(format!("{func}: {what} handle {handle} is not valid"));
    };
    match slots.get(idx) {
        Some(slot) if slot.tag == tag && tag != 0 => Ok(idx),
        // A tag mismatch is the interesting case: the slot exists but belongs
        // to a different resource now, so the handle outlived what it named.
        Some(_) => Err(format!(
            "{func}: {what} handle {handle} has been released (or belongs to a \
             previous window)"
        )),
        None => Err(format!("{func}: no {what} with handle {handle}")),
    }
}

/// Insert `value` into the first free slot, growing the registry if needed.
fn insert<T>(slots: &mut Vec<Slot<T>>, free: &mut Vec<usize>, value: T) -> i64 {
    let tag = next_tag();
    let slot = Slot {
        tag,
        value: Some(value),
    };

    match free.pop() {
        Some(idx) => {
            slots[idx] = slot;
            pack_handle(tag, idx)
        }
        None => {
            slots.push(slot);
            pack_handle(tag, slots.len() - 1)
        }
    }
}

impl Renderer {
    pub(crate) fn canvas_index(&self, handle: i64, func: &str) -> Result<usize, String> {
        resolve(&self.canvases, handle, func, "canvas")
    }

    pub(crate) fn font_index(&self, handle: i64, func: &str) -> Result<usize, String> {
        // The default face is addressed by `0` and lives outside the tagging
        // scheme, since nothing ever allocated or can release it.
        if handle == 0 {
            return Ok(0);
        }
        resolve(&self.fonts, handle, func, "font")
    }

    /// Release a canvas or image, freeing its pixels immediately.
    ///
    /// Returns an error for a handle that was already released, so a
    /// double-free is reported rather than quietly dropping whatever has since
    /// taken the slot.
    pub(crate) fn release_canvas(&mut self, handle: i64) -> Result<(), String> {
        let idx = self.canvas_index(handle, "Graphics.release")?;

        if self.target == Some(idx) {
            return Err(
                "Graphics.release: the canvas is the current render target — \
                 call Graphics.setCanvas() first"
                    .into(),
            );
        }
        if self.canvases[idx].value.is_none() {
            return Err("Graphics.release: the canvas is on loan to a draw call".into());
        }

        self.canvases[idx] = Slot::free();
        self.free_canvases.push(idx);
        Ok(())
    }

    /// Release a font. The default face (handle `0`) cannot be released.
    pub(crate) fn release_font(&mut self, handle: i64) -> Result<(), String> {
        if handle == 0 {
            return Err("Graphics.releaseFont: the default font cannot be released".into());
        }
        let idx = self.font_index(handle, "Graphics.releaseFont")?;

        if self.st.font == idx {
            return Err(
                "Graphics.releaseFont: the font is currently selected — call \
                 Graphics.setFont(0) first"
                    .into(),
            );
        }
        // A saved state on the push stack can still name this font, and
        // restoring it would resolve to a free slot. Cheaper to check than to
        // debug later.
        if self.stack.iter().any(|s| s.names_font(idx)) {
            return Err(
                "Graphics.releaseFont: the font is held by a Graphics.push state \
                 — pop it first"
                    .into(),
            );
        }

        self.fonts[idx] = Slot::free();
        self.free_fonts.push(idx);
        Ok(())
    }

    /// How many canvases and fonts are currently allocated — what
    /// `Graphics.getStats` reports, and what a leak test asserts on.
    pub(crate) fn live_counts(&self) -> (i64, i64) {
        let canvases = self.canvases.iter().filter(|s| !s.is_free()).count();
        // Slot 0 is the default face and is not an allocation.
        let fonts = self.fonts.iter().skip(1).filter(|s| !s.is_free()).count();
        (canvases as i64, fonts as i64)
    }

    /// Bytes held by every live canvas and image.
    pub(crate) fn canvas_bytes(&self) -> i64 {
        self.canvases
            .iter()
            .filter_map(|s| s.value.as_ref())
            .map(|s| (s.buf.len() * std::mem::size_of::<u32>()) as i64)
            .sum()
    }
}
