//! System clipboard.
//!
//! The context is kept alive in a `thread_local` rather than rebuilt per call.
//! That is not just an optimisation: on X11 the clipboard is *owned* by a live
//! connection, and dropping it takes the copied text with it, so a short-lived
//! context would leave nothing for another application to paste.

use std::cell::RefCell;

use saule_sdk::saule_export;

thread_local! {
    static CLIPBOARD: RefCell<Option<arboard::Clipboard>> = const { RefCell::new(None) };
}

/// Run `f` against the process clipboard, creating it on first use.
fn with<R>(func: &str, f: impl FnOnce(&mut arboard::Clipboard) -> R) -> Result<R, String> {
    CLIPBOARD.with(|cell| {
        let mut guard = cell.borrow_mut();

        if guard.is_none() {
            *guard = Some(
                arboard::Clipboard::new()
                    .map_err(|e| format!("{func}: no clipboard available: {e}"))?,
            );
        }

        Ok(f(guard.as_mut().expect("just created")))
    })
}

/// `Clipboard.get()` — the clipboard's text, or `""` when it is empty or holds
/// something that isn't text (an image, a file list).
///
/// Empty rather than an error because "nothing to paste" is an ordinary state,
/// not a failure — a paste handler shouldn't need a `try` around it.
#[saule_export(class = "Clipboard", name = "get")]
fn clipboard_get() -> Result<String, String> {
    with("Clipboard.get", |clipboard| {
        clipboard.get_text().unwrap_or_default()
    })
}

/// `Clipboard.set(text)` — replace the clipboard contents.
#[saule_export(class = "Clipboard", name = "set")]
fn clipboard_set(text: String) -> Result<(), String> {
    with("Clipboard.set", |clipboard| {
        clipboard
            .set_text(text)
            .map_err(|e| format!("Clipboard.set: {e}"))
    })?
}

/// `Clipboard.hasText()` — whether there is text to paste.
#[saule_export(class = "Clipboard", name = "hasText")]
fn clipboard_has_text() -> Result<bool, String> {
    with("Clipboard.hasText", |clipboard| {
        clipboard.get_text().is_ok_and(|text| !text.is_empty())
    })
}
