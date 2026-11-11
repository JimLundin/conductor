---
construct: item.use, expr.path and type.path naming a consuming reader
status: accepted
task: review of P1 unit T (docs/subset/reviews/p1-uT.md, Ratchet and Spec and tool notes)
---

# Consuming readers stay out; bytes are split and matched

## Rule
A type or trait in `consuming_readers` in `subset.toml` is never imported
or named in a path. The default list is `bytes::Buf`. A consuming reader
reads by changing the buffer it reads from: `Buf` for `&[u8]` reassigns
the slice on every `get_*`, so in `(header.try_get_u16(),
header.try_get_u16())` the two reads mean two different fields only by
evaluation order. Byte parsing has one form in the subset, from the
bootstrap review §A7: split with `split_at_checked(n)?`, convert with
`try_into()`, and match the parts, recursing for a repeated record.

This adds no construct. It narrows what `subset check` accepts.

## Before
This passes `subset check` under version 5. The checker reads
`let mut header = message;` as a library value lent as `&mut` (0008(b)),
because `header` is never reassigned in the syntax. `Buf` reassigns it
inside each call. Measured in `docs/subset/reviews/p1-uT.md` (Ratchet).

```rust
use bytes::Buf;

fn header(message: &[u8]) -> Option<(u16, u16)> {
    let mut header = message;
    match (header.try_get_u16(), header.try_get_u16()) {
        (Ok(kind), Ok(len)) => Some((kind, len)),
        _ => None,
    }
}
```

## After
```rust
fn header(message: &[u8]) -> Option<Header> {
    let (kind, rest) = message.split_at_checked(2)?;
    let (len, _) = rest.split_at_checked(2)?;
    Some(Header {
        kind: u16::from_be_bytes(kind.try_into().ok()?),
        len: u16::from_be_bytes(len.try_into().ok()?),
    })
}
```

## How the checker finds it
- **A `use` of a listed reader**, as `use bytes::Buf;`, is flagged as
  `item.use`.
- **A path through one**, as `Buf::get_u16(&mut slice)` or
  `bytes::Buf::advance`, is flagged as `expr.path`, and a type path as
  `type.path`, so a listed type such as `std::io::Cursor` is caught where a
  signature or binding names it.

Paths match as in 0038: the segments written before the name agree with
the listed path, and `core::` and `alloc::` read as `std::`. The reader's
methods are not matched by name. A trait's methods need the trait in
scope, so the `use` is the place to stop it.

## Limits
- A reader reached without naming it passes. A library prelude glob that
  re-exports `Buf` is the case to watch; the glob itself is still
  `item.use.glob`. A type with consuming inherent methods, such as
  `bytes::Bytes::split_to`, passes unless the type is on the list.
- The list holds what the project has met. A new consuming reader joins by
  review, as `endpoints` grows under 0035.

## Weight
No new id and no tier. A finding is a violation and costs `outside`.
Boundary use: doesn't apply.

## Measurements
No code under `crates/` uses `bytes::Buf` on `main` at `7ea697b`. The unit
T review measured the `Buf` form of the STUN parser at 441.0 against
550.0 for the §A7 form (-19.8%) and did not adopt it, because it weakens
forward data flow and local reasoning. That saving is the reason to close
the gap: it is the one candidate over the 15% bar, and it was legal only
by accident.

## Qualities
1. Named states: kept.
2. Visible branching: kept.
3. Forward data flow: kept, and helped. A read returns the parts it split
   instead of changing the buffer.
4. Errors are values: kept. A short buffer is a `None` from
   `split_at_checked`.
5. Local reasoning: kept, and helped. Which field a read returns is
   written at the read, not decided by the order of evaluation.
6. One way: kept, and helped. Byte parsing has the §A7 form only.
7. Repetition is data: kept.

## Owner decision
Accepted on 2026-11-11, in subset version 6, as a checker change and not
a construct. The list of readers is config (`consuming_readers`), and a
finding points to the §A7 form.
