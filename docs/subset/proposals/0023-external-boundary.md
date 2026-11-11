---
construct: boundary rule over type.unit, expr.unit, expr.lit.bool, type.bool, expr.reference.mut
status: accepted
task: rule edge cases found while encoding 0005 and 0021 (Dioxus EventHandler<()>)
---

# Library shapes are allowed at the library boundary

## Rule
The subset governs **our** types and logic. Where an API from outside our
crates demands a shape the subset otherwise excludes, that shape is allowed
**at the boundary, and only there**:

- **Type arguments of a type we don't define**, for example
  `EventHandler<()>`, `Future<Output = ()>`, `oneshot::Sender<()>` and
  `watch::Receiver<bool>`.
- **Arguments to a function or method we don't define**, for example
  `OpenOptions::create(true)`, `watch::channel(false)` and
  `tx.send(())`.
- **Field values of a struct we don't define**, for example
  `InitializeCapabilities { experimental_api: Some(true), .. }`.

The value goes straight into the library or comes straight out of it. It
never becomes a field, a variant payload, a parameter or a return type of
our own items. A `bool` that comes out of a library is turned into an enum
or an `Option` where it is received.

This replaces the boundary parts of 0005 (signal channels) and 0021 (`bool`
literals). The two are now cases of this rule, not separate exceptions.
0008(b) (`&mut` for library APIs) is the same rule applied to mutability.

## Before
The rules for 0005 and 0021 listed specific positions. Two Dioxus
`EventHandler<()>` sites, and generic parameters filled with `bool`, fell
outside those lists.

## After
One rule: the shape is allowed in a type argument, call argument or field
value of something we don't define.

## Measurements
Not measured. The owner set the direction ("we need to make some
affordances when we interact with external libraries").

## Qualities
1. Named states: kept. Our own types still never hold `bool` or `()`.
2. Visible branching: kept.
3. Forward data flow: kept. Library `&mut` stays under 0008(b).
4. Errors are values: kept.
5. Local reasoning: kept. The library's signature explains the shape, and
   the shape is right where the library is called.
6. One way: helped. One rule for every boundary shape, instead of a list of
   positions per construct.

## Enforcement
The checker decides "we don't define it" by name, as it does elsewhere
(`compromises.md`). A reviewer checks that a boundary value doesn't flow on
into our types.

## Owner decision
Accepted on 2026-11-11 by direction: "we need to make some affordances when
we interact with external libraries".
