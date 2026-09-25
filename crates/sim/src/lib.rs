//! A local exchange so strategies can be developed against a real matching
//! engine (queue position, partial fills, adverse selection) without any network access.

pub mod matching;
pub mod venue;

pub use matching::{MatchEvent, MatchingEngine, NewOrder};
pub use venue::{VenueConfig, VenueSim};
