//! The engine: everything that runs on the trading thread.
//!
//! [`Engine`] is generic over the strategy and the venue, so the same code runs
//! against the exchange simulator, paper fills on live data, a replayed journal,
//! or a real order gateway. Nothing in here knows which market it is trading.

mod engine;
pub mod stats;
pub mod venue;

pub use engine::{Engine, EngineOptions, Outputs, run_loop};
pub use venue::{PaperVenue, SimGateway, Venue};
