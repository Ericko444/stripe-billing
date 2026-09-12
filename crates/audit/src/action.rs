/// What happened -- a closed enum that grows one variant at a time.
///
/// Empty for now: each module's vertical slice adds its own variant here
/// as that slice lands, the same discipline `service`'s `Reads` and
/// `Writes` façades already follow in the billing module -- grow one
/// method (there, variant, here) at a time, never arrive complete.
///
/// There is deliberately no free-form variant such as `Action::Other(&str)`.
/// That would let a caller name any action it likes, including one whose
/// name happens to embed the very secret this model exists to keep out.
/// The vocabulary is fixed by this crate, not by its callers -- which also
/// means a new auditable action is a change to this crate, not a
/// parameter a caller supplies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {}
