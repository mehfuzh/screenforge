//! Project schemas. `v4` is the current, nested-config format written by Screen Studio 4.x.
//! `v3` is the flat-config format written by Screen Studio 2.4 – 3.x; it is read and migrated
//! to `v4` on load (see [`migrate`]).

pub mod common;
pub mod migrate;
pub mod v3;
pub mod v4;

/// Current project schema version written to `project.json`.
pub const CURRENT_VERSION: &str = "4.0.0";
