//! Objects whose memory lives in a native package.
//!
//! A `#[saule_class]` in a package produces objects the interpreter holds
//! by an opaque pointer. Each [`ForeignObject`] owns one of the package's
//! references to its object and gives it back when dropped, so the package
//! frees the object exactly when the last Saule value holding it goes away.
//! Its [`ForeignClass`] is where a method call or a property read on the
//! object is dispatched.

use std::ffi::c_void;
use std::fmt;
use std::rc::Rc;

use crate::fxhash::FxHashMap;
use crate::value::Value;

/// The static field under which a native class keeps its constructor, so
/// that calling the class — `Image(…)` — finds it. Not a name a program can
/// write: a Saule identifier cannot contain `#`.
pub const NATIVE_CONSTRUCTOR: &str = "#new";

/// A class a native package defines, as far as its *objects* are
/// concerned. The class's static side — `Image.load(…)`, `Image(…)` — is an
/// ordinary [`ClassObject`](crate::value::ClassObject) in the package's
/// exports; this is what `image.method(…)` finds.
pub struct ForeignClass {
    pub name: String,
    /// The import name of the package that defines the class. An object is
    /// only ever handed back to this package.
    pub package: Rc<str>,
    /// Instance methods. Each is a callable taking the object as argument 0.
    pub methods: FxHashMap<String, Value>,
    /// Property reads (`image.width`). Called with the object alone.
    pub getters: FxHashMap<String, Value>,
    /// Property writes (`image.width = 3`). Called with the object and the
    /// new value.
    pub setters: FxHashMap<String, Value>,
}

impl fmt::Debug for ForeignClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<native class {} from {}>", self.name, self.package)
    }
}

/// One reference to an object a package owns.
pub struct ForeignObject {
    ptr: *mut c_void,
    pub class: Rc<ForeignClass>,
    release: unsafe extern "C" fn(*mut c_void),
}

impl ForeignObject {
    /// Take over a reference the package handed us.
    ///
    /// # Safety
    /// `ptr` must carry one reference the host now owns, to an object of
    /// `class`'s package, and `release` must be that package's
    /// `saule_object_release`. The library behind `release` must stay loaded
    /// for as long as this value lives — true for every package, since
    /// loaded libraries are never unloaded.
    pub unsafe fn adopt(
        ptr: *mut c_void,
        class: Rc<ForeignClass>,
        release: unsafe extern "C" fn(*mut c_void),
    ) -> Self {
        ForeignObject {
            ptr,
            class,
            release,
        }
    }

    /// The package's pointer — the object's identity. Two values holding
    /// the same object compare equal even when each owns its own reference.
    pub fn ptr(&self) -> *mut c_void {
        self.ptr
    }
}

impl Drop for ForeignObject {
    fn drop(&mut self) {
        // SAFETY: we own exactly one reference, per `adopt`'s contract, and
        // give it back exactly once.
        unsafe { (self.release)(self.ptr) };
    }
}

impl fmt::Debug for ForeignObject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<instance of {} at {:p}>", self.class.name, self.ptr)
    }
}
