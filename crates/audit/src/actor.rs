use uuid::Uuid;

/// Identifies the subject behind an [`Actor::User`] -- opaque to this crate.
///
/// A raw `Uuid`, not a re-export of a host module's own user id: the
/// identity module's user id type is that module's own decision, made
/// later, and `audit` must not depend on it any more than it depends on
/// `domain`'s `TenantId`. A host converts its own id to a `SubjectId` at
/// the boundary where it constructs an entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SubjectId(Uuid);

impl SubjectId {
    /// Wraps a raw `Uuid` as a `SubjectId`.
    pub fn new(id: Uuid) -> Self {
        Self(id)
    }

    /// Returns the underlying `Uuid`.
    pub fn as_uuid(&self) -> Uuid {
        self.0
    }
}

/// Who acted.
///
/// A closed enum carrying an **opaque** subject id, not a generic
/// parameter over the caller's own user type: `Audit<UserId>` would make
/// this crate learn what a user is, and the identity module -- this
/// crate's other consumer -- is precisely what it must not depend on.
///
/// There is deliberately no `Actor::Other(String)` escape hatch. That
/// would let a caller attach an arbitrary label to who acted, which is
/// exactly the kind of free-form field the model exists to rule out --
/// see the crate-level docs for the property this protects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Actor {
    /// A person, identified by the authenticating module's own subject id,
    /// translated to a [`SubjectId`] at that module's boundary.
    User(SubjectId),
    /// The system itself acted -- webhook processing, scheduled work, or
    /// any path with no authenticated caller behind it.
    System,
    /// Someone acted without authenticating -- a password reset requested
    /// or completed by whoever holds the address or the link. Distinct from
    /// `System`: a person caused it, the module just cannot say who. Carries
    /// no payload -- in particular no IP address, which is personal data the
    /// journal is designed never to hold.
    Anonymous,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subject_id_round_trips_through_as_uuid() {
        let id = Uuid::new_v4();
        let subject_id = SubjectId::new(id);
        assert_eq!(subject_id.as_uuid(), id);
    }

    #[test]
    fn actor_variants_are_distinguishable() {
        let user = Actor::User(SubjectId::new(Uuid::new_v4()));
        let system = Actor::System;
        assert_ne!(user, system);
    }
}
