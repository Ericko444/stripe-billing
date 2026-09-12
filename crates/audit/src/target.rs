/// What the action was performed on -- a closed enum that grows one
/// variant at a time, for the same reason [`Action`](crate::Action) does:
/// each module's vertical slice adds the variant it needs, as it lands.
///
/// A variant added here carries a typed, crate-local id -- never a raw
/// string, and never another module's id type re-exported (the same
/// reasoning as [`crate::TenantId`] and [`crate::SubjectId`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Target {}
