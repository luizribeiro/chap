//! Types used by context plugins.

/// A block of context contributed to a provider request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Segment {
    /// Stable identifier used for deduplication and ordering across recomputations.
    pub id: String,
    pub content: String,
    /// Lower-priority segments render earlier.
    pub priority: i32,
}

/// Contributes context assembled at the start of an agent run.
#[allow(async_fn_in_trait)]
pub trait Context {
    /// Returns the context segments currently contributed by this plugin.
    async fn segments(&self) -> Result<Vec<Segment>, String>;
}
