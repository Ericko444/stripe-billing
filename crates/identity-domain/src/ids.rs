use uuid::Uuid;

macro_rules! id_type {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub struct $name(Uuid);

        impl $name {
            /// Wraps a raw `Uuid`.
            pub fn new(id: Uuid) -> Self {
                Self(id)
            }

            /// Returns the underlying `Uuid`.
            pub fn as_uuid(&self) -> Uuid {
                self.0
            }
        }
    };
}

id_type!(
    /// Identifies a user -- a person, global across tenants.
    UserId
);

id_type!(
    /// Identifies a tenant, local to the identity module.
    ///
    /// A parallel type to the billing module's `domain::TenantId`, not a
    /// re-export: this crate may not depend on `domain`. The host converts
    /// between the two through the `Uuid`, in the one place that names both
    /// modules.
    TenantId
);

id_type!(
    /// Identifies a membership: one user's place, with one role, in one
    /// tenant.
    MembershipId
);

id_type!(
    /// Identifies a session row. Never sent to a client -- the client holds
    /// a `SplitToken`, which is how a session is found and proven.
    SessionId
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip_through_their_uuid() {
        let raw = Uuid::new_v4();
        assert_eq!(UserId::new(raw).as_uuid(), raw);
        assert_eq!(TenantId::new(raw).as_uuid(), raw);
        assert_eq!(MembershipId::new(raw).as_uuid(), raw);
        assert_eq!(SessionId::new(raw).as_uuid(), raw);
    }
}
