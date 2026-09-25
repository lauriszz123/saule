//! Package-defined classes: objects whose memory lives in the package and
//! that Saule programs hold, pass around, and call methods on.
//!
//! A `#[saule_class]` struct `T` lives in an [`SObject<T>`] — a reference
//! counted box shared between the package and the interpreter. The
//! interpreter owns some of those references (one per Saule value holding
//! the object) and gives each back through `saule_object_release` when the
//! value goes away, so a `T`'s `Drop` runs exactly when the last holder on
//! either side lets go. That is what makes a texture, a socket or a file a
//! proper Saule object rather than an `integer` index into a table nobody
//! ever clears.
//!
//! The value inside is a `RefCell`, so every method call borrows it for
//! just that call: `&self` methods share it, a `&mut self` method has it to
//! itself, and a callback that re-enters the same object mid-mutation gets
//! a Saule runtime error instead of aliased `&mut`.

use std::cell::{Ref, RefCell, RefMut};
use std::fmt;
use std::rc::Rc;

use saule_native_abi::{CValue, ObjectPtr, tag};

use crate::convert::{FromSaule, IntoSaule, require};

/// A struct exposed to Saule as a class. Implemented by `#[saule_class]`;
/// never by hand.
pub trait NativeClass: Sized + 'static {
    /// The Saule class name — the struct's name.
    const NAME: &'static str;
    #[doc(hidden)]
    fn __descriptor() -> &'static ClassDescriptor;
}

/// Per-class, type-erased operations, one `static` per `#[saule_class]`.
/// Its address doubles as the class's identity: two pointers to the same
/// descriptor are the same Rust type.
#[doc(hidden)]
pub struct ClassDescriptor {
    name: &'static str,
    retain: unsafe fn(ObjectPtr),
    release: unsafe fn(ObjectPtr),
}

impl ClassDescriptor {
    pub const fn of<T: 'static>(name: &'static str) -> Self {
        ClassDescriptor {
            name,
            retain: retain_impl::<T>,
            release: release_impl::<T>,
        }
    }
}

/// The allocation behind every object. `#[repr(C)]` with the descriptor
/// first, so a pointer to any `ObjectBox<T>` can be read as a pointer to
/// its descriptor without knowing `T` — which is how `saule_object_release`
/// finds the right destructor.
#[repr(C)]
struct ObjectBox<T> {
    descriptor: &'static ClassDescriptor,
    value: RefCell<T>,
}

unsafe fn retain_impl<T>(ptr: ObjectPtr) {
    // SAFETY: `ptr` came from `Rc::into_raw` on an `Rc<ObjectBox<T>>` and at
    // least one strong reference is still alive (the caller's).
    unsafe { Rc::increment_strong_count(ptr as *const ObjectBox<T>) };
}

unsafe fn release_impl<T>(ptr: ObjectPtr) {
    // SAFETY: `ptr` came from `Rc::into_raw` on an `Rc<ObjectBox<T>>`, and
    // the caller is giving back one strong reference it owned.
    drop(unsafe { Rc::from_raw(ptr as *const ObjectBox<T>) });
}

/// The descriptor of any object, whatever its type.
///
/// # Safety
/// `ptr` must point to a live `ObjectBox` created by this crate.
unsafe fn descriptor_of(ptr: ObjectPtr) -> &'static ClassDescriptor {
    // SAFETY: `ObjectBox` is `#[repr(C)]` with the descriptor reference first.
    unsafe { *(ptr as *const &'static ClassDescriptor) }
}

/// Give back one host-owned reference. What the generated
/// `saule_object_release` export calls.
///
/// # Safety
/// `ptr` must be an object this package handed the host, and the host must
/// own the reference being released.
#[doc(hidden)]
pub unsafe fn release_object(ptr: ObjectPtr) {
    if ptr.is_null() {
        return;
    }
    // A `Drop` impl that panics must not unwind into the interpreter: this
    // is an `extern "C"` call, so unwinding would abort the process. The
    // object is gone either way; the panic message has already been printed.
    let _ = std::panic::catch_unwind(|| {
        // SAFETY: forwarded from the caller.
        let d = unsafe { descriptor_of(ptr) };
        unsafe { (d.release)(ptr) };
    });
}

/// A shared handle to an object of a `#[saule_class]`.
///
/// Take one as a parameter to keep the object beyond the call — store it in
/// your own state, say, as the current render target — and return one to
/// hand Saule an object you also keep. Clone it freely; it is an `Rc`.
pub struct SObject<T: NativeClass> {
    rc: Rc<ObjectBox<T>>,
}

impl<T: NativeClass> SObject<T> {
    /// Move `value` into a new object.
    pub fn new(value: T) -> Self {
        SObject {
            rc: Rc::new(ObjectBox {
                descriptor: T::__descriptor(),
                value: RefCell::new(value),
            }),
        }
    }

    /// Borrow the value. Fails, rather than panicking, if a `&mut self`
    /// method on the same object is still running further up the stack.
    pub fn borrow(&self) -> Result<Ref<'_, T>, String> {
        self.rc.value.try_borrow().map_err(|_| {
            format!(
                "this {} is being modified by a call that has not returned yet",
                T::NAME
            )
        })
    }

    /// Borrow the value mutably. Fails if any other call on the same object
    /// is still running further up the stack.
    pub fn borrow_mut(&self) -> Result<RefMut<'_, T>, String> {
        self.rc.value.try_borrow_mut().map_err(|_| {
            format!(
                "this {} is in use by a call that has not returned yet",
                T::NAME
            )
        })
    }

    /// Whether two handles refer to the same object.
    pub fn ptr_eq(a: &Self, b: &Self) -> bool {
        Rc::ptr_eq(&a.rc, &b.rc)
    }
}

impl<T: NativeClass> Clone for SObject<T> {
    fn clone(&self) -> Self {
        SObject {
            rc: Rc::clone(&self.rc),
        }
    }
}

impl<T: NativeClass> fmt::Debug for SObject<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SObject<{}>({:p})", T::NAME, Rc::as_ptr(&self.rc))
    }
}

impl<T: NativeClass> FromSaule for SObject<T> {
    fn from_saule(args: &[CValue], idx: usize, func: &str, param: &str) -> Result<Self, String> {
        let v = require(args, idx, func, param)?;
        let Some(ptr) = v.as_object().filter(|p| !p.is_null()) else {
            return Err(format!("{func}: argument `{param}` must be a {}", T::NAME));
        };
        // SAFETY: the host passes a package only the objects it created, so
        // `ptr` is one of ours and is alive for the call.
        let d = unsafe { descriptor_of(ptr) };
        if !std::ptr::eq(d, T::__descriptor()) {
            return Err(format!(
                "{func}: argument `{param}` must be a {}, got a {}",
                T::NAME,
                d.name
            ));
        }
        // SAFETY: same allocation, and it is an `ObjectBox<T>` — the
        // descriptor check just proved it. Take our own reference: the
        // host's is only borrowed for the call.
        unsafe {
            (d.retain)(ptr);
            Ok(SObject {
                rc: Rc::from_raw(ptr as *const ObjectBox<T>),
            })
        }
    }
}

impl<T: NativeClass> IntoSaule for SObject<T> {
    /// Hands the host this handle's reference.
    fn into_saule(self) -> CValue {
        let ptr = Rc::into_raw(self.rc) as ObjectPtr;
        CValue::object(ptr, T::NAME)
    }
}

/// An object of *some* class of this package — what reading a table or
/// calling a callback gives back when the value is an object. Narrow it
/// with [`SAnyObject::downcast`].
pub struct SAnyObject {
    ptr: ObjectPtr,
}

impl SAnyObject {
    /// Take a reference to an object the host lent us.
    ///
    /// # Safety
    /// `ptr` must be a live object of this package.
    pub(crate) unsafe fn retain_borrowed(ptr: ObjectPtr) -> Self {
        // SAFETY: forwarded from the caller.
        unsafe { (descriptor_of(ptr).retain)(ptr) };
        SAnyObject { ptr }
    }

    fn descriptor(&self) -> &'static ClassDescriptor {
        // SAFETY: we hold a reference, so the object is alive.
        unsafe { descriptor_of(self.ptr) }
    }

    /// The object's Saule class name.
    pub fn class_name(&self) -> &'static str {
        self.descriptor().name
    }

    /// This object as a `T`, if it is one.
    pub fn downcast<T: NativeClass>(&self) -> Option<SObject<T>> {
        if !std::ptr::eq(self.descriptor(), T::__descriptor()) {
            return None;
        }
        // SAFETY: the descriptor check proves the type; take a new reference
        // for the returned handle.
        unsafe {
            (self.descriptor().retain)(self.ptr);
            Some(SObject {
                rc: Rc::from_raw(self.ptr as *const ObjectBox<T>),
            })
        }
    }

    /// A `CValue` carrying a new reference, for the host to take over.
    pub(crate) fn to_owned_cvalue(&self) -> CValue {
        let d = self.descriptor();
        // SAFETY: we hold a reference, so retaining another is sound.
        unsafe { (d.retain)(self.ptr) };
        CValue::object(self.ptr, d.name)
    }
}

impl Clone for SAnyObject {
    fn clone(&self) -> Self {
        // SAFETY: we hold a reference, so the object is alive.
        unsafe { SAnyObject::retain_borrowed(self.ptr) }
    }
}

impl Drop for SAnyObject {
    fn drop(&mut self) {
        // SAFETY: we own exactly one reference.
        unsafe { (self.descriptor().release)(self.ptr) };
    }
}

impl fmt::Debug for SAnyObject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SAnyObject<{}>({:p})", self.class_name(), self.ptr)
    }
}

impl<T: NativeClass> From<SObject<T>> for SAnyObject {
    fn from(o: SObject<T>) -> Self {
        SAnyObject {
            ptr: Rc::into_raw(o.rc) as ObjectPtr,
        }
    }
}

/// Decode an object argument of any class of this package.
pub(crate) fn any_from_cvalue(v: &CValue) -> Option<SAnyObject> {
    if v.tag != tag::OBJECT {
        return None;
    }
    let ptr = v.as_object().filter(|p| !p.is_null())?;
    // SAFETY: the host passes a package only its own live objects.
    Some(unsafe { SAnyObject::retain_borrowed(ptr) })
}
