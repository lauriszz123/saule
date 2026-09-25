# saule-sdk

Write a Saule native package in ordinary Rust. You annotate structs,
`impl` blocks, enums and functions; the SDK generates every `extern "C"`
shim and compiles a description of the package — its classes, every
signature, your doc comments — **into the library itself**. The one file you
build is the whole package: copy it into `~/.saule/native_packages/` and
Saule programs can import it, fully typed, with editor completion and hover.

```rust
use saule_sdk::prelude::*;

saule_package! {
    name = "gfx",
    version = "0.1.0",
    doc = "Images in memory.",
    classes { Gfx = "Loading and saving images." }
}

/// An RGBA image held in memory.
#[saule_class]
pub struct Image { w: i64, h: i64, pixels: Vec<u32> }

#[saule_methods]
impl Image {
    /// A blank image.
    pub fn new(w: i64, h: i64) -> Self {
        Image { w, h, pixels: vec![0; (w * h) as usize] }
    }

    /// Width in pixels.
    #[saule(getter)]
    pub fn width(&self) -> i64 { self.w }

    /// Paint every pixel.
    pub fn fill(&mut self, color: i64) { self.pixels.fill(color as u32) }

    /// Copy `other` over this image.
    pub fn blit(&mut self, other: &Image) { /* … */ }
}

/// How to scale.
#[saule_enum]
pub enum Filter { Nearest, Linear }

/// Decode a PNG.
#[saule_export(class = "Gfx")]
fn load(path: &str) -> Result<Image, String> { /* … */ }
```

What a Saule program sees:

```luau
import * from "gfx"

local img: Image = Gfx.load("a.png")   -- a real class, not an integer handle
local copy = Image(img.width, 64)       -- constructor
copy.blit(img)
copy.fill(0xff0000ff)
copy.fill("red")                        -- error: expects `integer`, got `string`
```

When the last Saule value holding an `Image` goes away, the `Image` is
dropped — its `Drop` runs in your package.

## The pieces

| Item | Role |
|------|------|
| `saule_package!` | The package's import name and version, optionally a `doc` and the docs of static-only classes. Exactly one per package. |
| `#[saule_export(class = "…")]` | A free function as a static member of a class: `Gfx.load(…)`. `name = "…"` renames it; otherwise the Rust name in `lowerCamelCase`. |
| `#[saule_class]` | A struct as a Saule class whose objects programs hold. |
| `#[saule_methods]` | Exports the `pub fn`s of a class's `impl`. See below. |
| `#[saule_enum]` | A fieldless enum as a Saule enum with the same variants. |
| `SObject<T>` | A shared handle to an object: take one to keep an object past the call, return one to hand out an object you also keep. |
| `SAnyObject` | An object of some class of this package, as read out of a table or a callback's result; narrow it with `downcast`. |
| `STable`, `SFunction`, `SValue`, … | Saule tables and callbacks, worked on in place through the host. |

### Methods

Every `pub fn` in a `#[saule_methods]` block is exported, and its role
follows from its signature:

| Rust | Saule |
|------|-------|
| `pub fn new(…) -> Self` | `Image(…)` — the constructor |
| `pub fn width(&self) -> i64` | `img.width()` |
| `pub fn fill(&mut self, …)` | `img.fill(…)` |
| `pub fn load(…) -> Result<Self, E>` (no `self`) | `Image.load(…)` |

`#[saule(...)]` adjusts that: `getter` / `setter` make a property
(`img.width`, `img.width = 3`), `constructor` makes a differently named
function the constructor, `name = "…"` renames, `sig(f = "fn(T) -> T")`
types a callback parameter, and `skip` leaves a `pub fn` out. Method names
are converted from `snake_case` to the `lowerCamelCase` Saule uses.

Objects are borrowed for each call: `&self` methods share the object, and a
`&mut self` method has it to itself. A callback that re-enters an object
while a `&mut self` method on it is still running gets a Saule runtime
error, never aliased `&mut`.

## Type mapping

| Rust | Saule |
|------|-------|
| `i8`…`i64`, `u8`…`u64`, `isize`, `usize` | `integer` (range-checked) |
| `f32`, `f64` | `float` |
| `bool` | `boolean` |
| `String`, `&str` | `string` |
| `Option<T>`, `Option<&T>` | `T?` (and may be omitted as a trailing argument) |
| `Vec<T>` | `table<T>` (copied) |
| `HashMap<K, V>`, `BTreeMap<K, V>` | `table<K, V>` (copied) |
| `STable<T>` | `table<T>` (the Saule table itself) |
| `SFunction` + `sig(f = "…")` | the declared function type |
| `SValue` | `any` |
| a `#[saule_class]` `T` | `T` — taken as `&T`, `&mut T` or `SObject<T>`; returned as `T` or `SObject<T>` |
| a `#[saule_enum]` `E` | `E` |
| `()` | `nil` |

A function may return `Result<T, E>` (an `Err` becomes a Saule runtime error
at the call) or a tuple `(A, B, …)` for multiple returns. A panic becomes a
runtime error too, naming the member and where it panicked.

## Building and installing

```toml
[lib]
crate-type = ["cdylib"]

[dependencies]
saule-sdk = "…"
```

```sh
cargo build --release
cp target/release/libgfx.dylib ~/.saule/native_packages/   # .so / .dll elsewhere
```

That is all: there is no manifest to generate. Saule reads the package's
description out of the file without loading it, so a program that imports
it is type-checked — and the editor completes and documents it — before any
of your code runs. A package built against a different `saule-sdk` ABI is
refused at its `import`, with both versions named.

See `crates/saule-native-fixture` for every feature in one small package,
and `crates/saule-engine-lib` for a large one.
