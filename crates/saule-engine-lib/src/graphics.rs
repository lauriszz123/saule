//! Graphics module — drawing primitives backed by a software rasterizer.
//!
//! Each function is an `extern "C"` symbol named in the package manifest and
//! draws into the framebuffer owned by [`crate::state`]. Geometry is rendered
//! immediately into the buffer; `Graphics.present` pushes it to the window.

use saule_native_abi::{return_error, CValue, NativeSymbolFn};

use crate::args::Args;
use crate::state;

/// Run `body`, converting an `Err(msg)` into the ABI's error convention
/// (non-zero return code + `ERR` `out` value). Keeps every exported symbol
/// a tidy one-liner and guarantees `out` is always written.
fn dispatch(out: &mut CValue, body: impl FnOnce() -> Result<CValue, String>) -> i32 {
    let (value, code) = match body() {
        Ok(v) => (v, 0),
        Err(msg) => (return_error(&msg), 1),
    };
    *out = value;
    code
}

/// `Graphics.setColor(r: float, g: float, b: float) -> nil` — set the colour
/// used by subsequent `circle` / `rectangle` calls. Channels are `0.0..=1.0`.
///
/// # Safety
/// ABI entry point — see [`saule_native_abi`] for the pointer contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn saule_engine_graphics_set_color(
    args: *const CValue,
    argc: usize,
    out: *mut CValue,
) -> i32 {
    let a = unsafe { Args::new(args, argc) };
    let out = unsafe { &mut *out };
    dispatch(out, || {
        a.expect_arity("Graphics.setColor", 3)?;
        let r = a.float(0)?;
        let g = a.float(1)?;
        let b = a.float(2)?;
        state::with(|e| e.set_color(r, g, b))?;
        Ok(CValue::nil())
    })
}

/// `Graphics.circle(mode: string, x: float, y: float, radius: float) -> nil`
///
/// # Safety
/// ABI entry point — see [`saule_native_abi`] for the pointer contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn saule_engine_graphics_circle(
    args: *const CValue,
    argc: usize,
    out: *mut CValue,
) -> i32 {
    let a = unsafe { Args::new(args, argc) };
    let out = unsafe { &mut *out };
    dispatch(out, || {
        a.expect_arity("Graphics.circle", 4)?;
        let mode = a.string(0)?;
        let x = a.float(1)?;
        let y = a.float(2)?;
        let radius = a.float(3)?;
        state::with(|e| e.circle(&mode, x, y, radius))?;
        Ok(CValue::nil())
    })
}

/// `Graphics.rectangle(mode: string, x: float, y: float, w: float, h: float) -> nil`
///
/// # Safety
/// ABI entry point — see [`saule_native_abi`] for the pointer contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn saule_engine_graphics_rectangle(
    args: *const CValue,
    argc: usize,
    out: *mut CValue,
) -> i32 {
    let a = unsafe { Args::new(args, argc) };
    let out = unsafe { &mut *out };
    dispatch(out, || {
        a.expect_arity("Graphics.rectangle", 5)?;
        let mode = a.string(0)?;
        let x = a.float(1)?;
        let y = a.float(2)?;
        let w = a.float(3)?;
        let h = a.float(4)?;
        state::with(|e| e.rectangle(&mode, x, y, w, h))?;
        Ok(CValue::nil())
    })
}

/// `Graphics.clear(r: float, g: float, b: float) -> nil` — begin a frame by
/// clearing the framebuffer to the given colour. Call at the top of each loop
/// iteration.
///
/// # Safety
/// ABI entry point — see [`saule_native_abi`] for the pointer contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn saule_engine_graphics_clear(
    args: *const CValue,
    argc: usize,
    out: *mut CValue,
) -> i32 {
    let a = unsafe { Args::new(args, argc) };
    let out = unsafe { &mut *out };
    dispatch(out, || {
        a.expect_arity("Graphics.clear", 3)?;
        let r = a.float(0)?;
        let g = a.float(1)?;
        let b = a.float(2)?;
        state::with(|e| e.clear(r, g, b))?;
        Ok(CValue::nil())
    })
}

/// `Graphics.present() -> nil` — end a frame: push the framebuffer to the
/// window, pump events, and apply the 60 FPS frame limit. Call at the bottom
/// of each loop iteration.
///
/// # Safety
/// ABI entry point — see [`saule_native_abi`] for the pointer contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn saule_engine_graphics_present(
    args: *const CValue,
    argc: usize,
    out: *mut CValue,
) -> i32 {
    let a = unsafe { Args::new(args, argc) };
    let out = unsafe { &mut *out };
    dispatch(out, || {
        a.expect_arity("Graphics.present", 0)?;
        state::with(|e| e.present())??;
        Ok(CValue::nil())
    })
}

/// Compile-time assertion that every exported symbol matches the ABI's
/// frozen function signature. If a signature ever drifts this fails to
/// build instead of mis-loading at runtime.
const _: [NativeSymbolFn; 5] = [
    saule_engine_graphics_set_color,
    saule_engine_graphics_circle,
    saule_engine_graphics_rectangle,
    saule_engine_graphics_clear,
    saule_engine_graphics_present,
];
