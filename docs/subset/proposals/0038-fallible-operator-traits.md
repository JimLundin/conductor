---
construct: item.use, expr.path, type.path and expr.method_call naming a fallible operator trait
status: accepted
task: review of P1 unit T (docs/subset/reviews/p1-uT.md, Rulings)
---

# Fallible operator traits are never named or called as methods

## Rule
Arithmetic goes only through checked methods (`docs/subset/examples.md`
§2). A trait in `fallible_operators` in `subset.toml` is never imported,
never named in a path, and its methods are never called by name. The
default list is `std::ops::{Add, Sub, Mul, Div, Rem, Neg, Shl, Shr}` and
their `*Assign` forms, each with its method (`add`, `add_assign`, and so
on). The fix is the `checked_*` or `saturating_*` method. The total
bitwise traits (`BitAnd`, `BitOr`, `BitXor` and their `*Assign` forms) and
`Not` stay allowed, so `BitXor::bitxor` remains the form for an XOR
(bootstrap review §A5).

This adds no construct. It narrows what `subset check` accepts.

## Before
Each of these passes `subset check` under version 5. Each panics on
overflow in a debug build and wraps in a release build, which is what
`checked_add` exists to prevent.

```rust
use std::ops::{Add, Shl};

fn total(a: u64, b: u64) -> u64 {
    a.add(b)
}

fn sum(a: u64, b: u64) -> u64 {
    Add::add(a, b)
}

fn high(bits: u64) -> u64 {
    bits.shl(70)
}
```

## After
```rust
fn total(a: u64, b: u64) -> Option<u64> {
    a.checked_add(b)
}

fn high(bits: u64) -> Option<u64> {
    bits.checked_shl(70)
}
```

## How the checker finds it
The checker has no types, so it works from names.

- **A `use` of a listed trait**, as `use std::ops::Add;` or
  `use std::ops::{Add as _};`, is flagged as `item.use`.
- **A path through a listed trait**, as `Add::add(a, b)` or
  `std::ops::Add::add(a, b)`, is flagged as `expr.path`, and a type path
  through one as `type.path`. A qualified path such as
  `<u64 as Add>::add` is already outside the subset
  (`expr.path.qualified`).
- **A method call** is flagged as `expr.method_call` when its name is the
  method of a listed trait **and** the file imports that trait, by name or
  through a glob of its module. Calling a trait method needs the trait in
  scope, and the prelude doesn't bring in the operator traits, so a call
  without the import doesn't compile. The import is flagged as well, so
  the finding at the call says what to change there.

A path matches a listed trait when its segment names the trait and the
segments written before it agree with the trait's path, so `ops::Add`
matches `std::ops::Add` and `other::Add` doesn't. `core::` and `alloc::`
read as `std::`. A bare name that the file defines or imports from a path
of ours is ours, not the library trait.

## Limits
- A method call is matched by name. A method `add` on a type of ours
  passes unless the file also imports `Add`, and then it is flagged even
  though it isn't the operator. Rename the method if that happens.
- A trait brought into scope another way, such as a library prelude that
  re-exports it through a glob of that library, is not seen, so its method
  calls pass. The glob import itself is still `item.use.glob`.
- An operator trait in a bound (`T: Add`) is not checked here. Type
  parameters already allow only the codec bounds of 0013, so such a bound
  is a violation anyway.

## Weight
No new id and no tier. A finding is a violation and costs `outside`.
Boundary use: doesn't apply.

## Measurements
No code under `crates/` uses a listed trait on `main` at `7ea697b`, so the
rule changes no score. The finding count is in the version 6 rewrite
report.

## Qualities
1. Named states: kept.
2. Visible branching: kept, and helped. An overflow becomes a `None` the
   caller matches, not a panic or a silent wrap.
3. Forward data flow: kept.
4. Errors are values: kept, and helped, for the same reason.
5. Local reasoning: kept.
6. One way: kept, and helped. Arithmetic has one form, the checked method,
   instead of that form plus a trait-method spelling that the operator ban
   didn't reach.
7. Repetition is data: kept.

## Owner decision
Accepted on 2026-11-11, in subset version 6, as a checker change and not
a construct. The trait list is config (`fallible_operators`), so the tool
stays reusable. Method calls are matched by name only where the trait is
imported, and the limit is documented.
