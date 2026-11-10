---
construct: expr.tuple, pat.tuple, type.tuple
status: accepted
task: subset examples (docs/subset/examples.md §5)
---

# Tuples

## Rule
Tuples are for matching on several values at once (`match (state, event)`)
and for short-lived pairs inside a function. A value that outlives its
expression, or appears in a public signature, is a named struct.

## Before
Nested matches, one level per value (examples.md §5 before version 1).

## After
```rust
match (self, event) {
    (Link::Disconnected { attempts }, LinkEvent::Dialled) => Link::Connecting { attempts },
    (_, LinkEvent::Opened(session)) => Link::Live { session },
    …
}
```

## Measurements
Not measured. The owner accepted tuples directly.

## Qualities
1. Named states: kept, because the rule keeps lasting values in named structs.
2. Visible branching: kept. Every combination is an arm.
3. Forward data flow: kept.
4. Errors are values: kept.
5. Local reasoning: kept. The arms read as a transition table.
6. One way: kept. A multi-value decision is one `match` on a tuple.

## Owner decision
Accepted on 2026-11-10.
