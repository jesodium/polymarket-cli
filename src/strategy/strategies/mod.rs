//! Strategy plugins. Each strategy is a self-contained file implementing
//! [`crate::strategy::Strategy`]. Add a new file here and register it in
//! [`crate::strategy::registry`] — no changes to the core app are needed.

pub(crate) mod mean_reversion;
pub(crate) mod momentum;
pub(crate) mod tp_sl;
