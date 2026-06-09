//! Paper trading: simulated order execution against live market data.
//!
//! Fully isolated from real wallets and the authenticated CLOB client. State
//! lives in a local JSON file (see [`store`]); fills are simulated by the
//! pure functions in [`engine`] using quotes fetched through the existing
//! unauthenticated CLOB/Gamma clients (see [`quotes`]).

pub(crate) mod engine;
pub(crate) mod quotes;
pub(crate) mod store;
pub(crate) mod types;
