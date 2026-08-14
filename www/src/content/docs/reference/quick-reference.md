---
title: "Quick Reference"
description: "The tables below are for looking something up at a glance. For the exact shape of every construct, see the Grammar."
---

<!-- Generated from README.md by `npm run sync-docs`. Edit that file, not this one. -->

The tables below are for looking something up at a glance. For the exact shape
of every construct, see the [Grammar](/saule/reference/grammar/).

### Keywords

| Keyword | Purpose |
|---|---|
| `class` | Declare a class |
| `interface` | Declare an interface |
| `enum` | Declare an enum |
| `fn` | Declare a function or method |
| `extends` | Inherit from a class |
| `implements` | Fulfill one or more interfaces |
| `super` | Call the parent's `init` |
| `self` | Reference the current instance |
| `static` | Declare a class-level member |
| `local` | Declare a private member or variable |
| `export` | Make a file member publicly importable |
| `import` | Import from another file |
| `return` | Return a value from a function |
| `throw` | Raise an error |
| `try` | Begin an error-handled block |
| `catch` | Handle a thrown error |
| `for` | Begin a loop |
| `while` | Begin a while loop |
| `repeat` | Begin a repeat-until loop |
| `until` | End condition for repeat loop |
| `break` | Exit a loop |
| `continue` | Skip to the next iteration |
| `if / elseif / else / end` | Conditional logic |
| `match` | Begin a pattern-matching expression |
| `case` | Introduce a pattern arm inside `match` |
| `when` | Attach a guard condition to a `case`, or start a `when(...)` pipeline |
| `then` | Begin a `match` arm body / `if` branch |
| `nil` | Absence of value |
| `true / false` | Boolean literals |

### Literals

| Literal | Type | Notes |
|---|---|---|
| `42` | `integer` | |
| `0xFF`, `0b1010` | `integer` | `_` may separate digits: `0xFF_80_00` |
| `3.14` | `float` | |
| `.5` | `float` | The integer part may be omitted |
| `10f`, `10F` | `float` | Suffix form of `10.0` |
| `"text"` | `string` | |
| `true`, `false` | `boolean` | |
| `nil` | — | Only assignable to a nullable (`T?`) slot |

### Operators

| Operator | Meaning |
|---|---|
| `?.` | Safe member access |
| `??` | Null coalescing fallback |
| `!` | Force unwrap nullable |
| `..` | String concatenation |
| `#` | Length of table or string |
| `==`, `!=` | Equality checks |
| `>`, `<`, `>=`, `<=` | Comparisons |
| `and`, `or`, `not` | Boolean logic |
| `+`, `-`, `*`, `/`, `%` | Arithmetic (`/` on two `integer`s truncates) |
| `^` | Exponentiation (right-associative, binds tighter than unary `-`) |
| `&`, `\|`, `~`, `<<`, `>>` | Bitwise and / or / xor / shifts — `integer` only |
| `~a` | Bitwise complement (prefix `~`; infix `~` is xor) |
| `+=`, `-=`, `*=`, `/=`, `%=`, `^=`, `..=`, `&=`, `\|=`, `<<=`, `>>=` | Compound assignment (no `~=` — that is Lua's `!=`) |
| `:` | Pipeline stage call inside `when(...)` |
| `int()` | Cast float to integer, truncates toward zero |
| `float()` | Cast integer to float, always safe |
