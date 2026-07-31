# vector-math

Operator overloading, end to end. A `Vec2` that adds, subtracts, multiplies,
negates, compares and prints like a built-in number, and a `Path` that answers
`#` and joins with `..`.

Run with:

```sh
saule run
```

## What it shows

Every operator a class can take over is an interface, and the method the
interface names is what runs:

| File | Interfaces | Operators |
|---|---|---|
| `src/vec2.sau` | `OpAdd` `OpSub` `OpMul` `OpNeg` `OpEq` `OpToString` | `+` `-` `*` `-a` `==` `tostring` |
| `src/path.sau` | `OpLen` `OpConcat` `OpToString` | `#` `..` `tostring` |

The result type comes from the method's own signature, so `a + b` is a `Vec2`
and fills a `Vec2` slot with no cast. `Main.simulate` leans on that: a Euler
integration step reads like the physics it models once `+` and scaling mean
something for vectors.

## Two things worth noticing

**`==` stops meaning identity.** Without `OpEq`, two separately built vectors
holding the same numbers compare unequal, because the default is pointer
identity. `Vec2.equals` fixes that — but note that `v == nil` still works, since
a `nil` operand never reaches an overload.

**`OpConcat` takes `..` over completely.** `Path` joins with `..`, which means
`"path = " .. somePath` is a *type error*: `..` is right-associative, so the
`Path` lands on the left and its `concat` runs, and `concat` wants a `Path`.
Use `tostring(somePath)` when you want the string. `Vec2` has no `OpConcat`, so
it interpolates directly — `"a = " .. a` is fine and goes through `OpToString`.

## Files

- `src/vec2.sau` — the vector type and its six operators
- `src/path.sau` — `#` and `..` on a list of points
- `src/main.sau` — a falling-body simulation and a path walk
