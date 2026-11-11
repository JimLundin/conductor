//! Reasons named after the accepted proposals' rules.

const COMPOUND_ASSIGNMENT: &str =
    "compound assignment is not allowed; write `x = x.checked_add(1)?` (proposal 0008)";
const INTERIOR_MUTABILITY: &str = "interior mutability stays out of the subset (proposal 0008)";

/// The reason for an id that a proposal names as never allowed, in place of
/// the id's plain meaning.
pub fn forbidden(id: &str) -> Option<&'static str> {
    match id {
        _ if id.starts_with("expr.binary.") && id.ends_with("_assign") => Some(COMPOUND_ASSIGNMENT),
        _ if id.starts_with("type.interior_mut.") || id.starts_with("expr.interior_mut.") => {
            Some(INTERIOR_MUTABILITY)
        }
        _ => None,
    }
}
