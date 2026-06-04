//! Window module — real OS window lifecycle and the open/event state that
//! drives a Saule-side game loop.

use saule_native_abi::{return_error, CValue, NativeSymbolFn};

use crate::args::Args;
use crate::state;

/// Write the dispatch result into `out`.
fn dispatch(out: &mut CValue, body: impl FnOnce() -> Result<CValue, String>) -> i32 {
    let (value, code) = match body() {
        Ok(v) => (v, 0),
        Err(msg) => (return_error(&msg), 1),
    };
    *out = value;
    code
}

/// `Window.create(width: integer, height: integer, title: string?) -> nil`
///
/// Opens a real OS window and allocates its framebuffer. The optional third
/// argument sets the title bar (defaults to `"Saule"`).
///
/// # Safety
/// ABI entry point — see [`saule_native_abi`] for the pointer contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn saule_engine_window_create(
    args: *const CValue,
    argc: usize,
    out: *mut CValue,
) -> i32 {
    let a = unsafe { Args::new(args, argc) };
    let out = unsafe { &mut *out };
    dispatch(out, || {
        if a.len() != 2 && a.len() != 3 {
            return Err(format!(
                "Window.create expects 2 or 3 arguments, got {}",
                a.len()
            ));
        }
        let width = a.integer(0)?;
        let height = a.integer(1)?;
        let title = if a.len() == 3 {
            a.string(2)?
        } else {
            "Saule".to_string()
        };

        state::create(width, height, &title)?;
        crate::timer::reset_clock();

        Ok(CValue::nil())
    })
}

/// `Window.isOpen() -> boolean` — the game-loop condition. False once the
/// user closes the window or holds Escape.
///
/// # Safety
/// ABI entry point — see [`saule_native_abi`] for the pointer contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn saule_engine_window_is_open(
    args: *const CValue,
    argc: usize,
    out: *mut CValue,
) -> i32 {
    let a = unsafe { Args::new(args, argc) };
    let out = unsafe { &mut *out };
    dispatch(out, || {
        a.expect_arity("Window.isOpen", 0)?;
        // If no window exists, the loop is over.
        let open = state::with(|e| e.is_open()).unwrap_or(false);
        Ok(CValue::boolean(open))
    })
}

/// `Window.pollEvents() -> nil` — pump the OS event queue once per frame so
/// `isOpen` and input stay fresh at the top of the loop.
///
/// # Safety
/// ABI entry point — see [`saule_native_abi`] for the pointer contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn saule_engine_window_poll_events(
    args: *const CValue,
    argc: usize,
    out: *mut CValue,
) -> i32 {
    let a = unsafe { Args::new(args, argc) };
    let out = unsafe { &mut *out };
    dispatch(out, || {
        a.expect_arity("Window.pollEvents", 0)?;
        state::with(|e| e.poll_events())?;
        Ok(CValue::nil())
    })
}

/// `Window.close() -> nil` — close the window and end the loop.
///
/// # Safety
/// ABI entry point — see [`saule_native_abi`] for the pointer contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn saule_engine_window_close(
    args: *const CValue,
    argc: usize,
    out: *mut CValue,
) -> i32 {
    let a = unsafe { Args::new(args, argc) };
    let out = unsafe { &mut *out };
    dispatch(out, || {
        a.expect_arity("Window.close", 0)?;
        state::close();
        Ok(CValue::nil())
    })
}

const _: [NativeSymbolFn; 4] = [
    saule_engine_window_create,
    saule_engine_window_is_open,
    saule_engine_window_poll_events,
    saule_engine_window_close,
];
