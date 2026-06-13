//! Copy-trading: mirror another wallet's trades onto the paper account or the
//! live CLOB.
//!
//! You follow one or more wallets;
//! the [`engine`] polls each one's recent trade activity through the Data API
//! and replicates qualifying trades with your own fixed size, capped and
//! price-filtered per your [`config`]. Roster lives on disk; runtime state
//! (what we've already seen, counters) lives in the engine.

pub(crate) mod config;
pub(crate) mod engine;
