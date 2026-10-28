//! Editing agent: the user asks in plain words ("cut the first five seconds", "speed up the
//! typing to 2×") and Claude edits the project through tools.
//!
//! * [`tools`]: what Claude can do to a [`tools::Workspace`] (a project and its transcript):
//!   read the timeline, cut, change speed, zoom, and patch any field of `project.json`.
//! * [`client`]: the Claude Messages API (through the ScreenForge backend, or directly with a
//!   local key) and model prices.
//! * [`agent`]: one user turn: call Claude, run the tools it asks for, repeat until it answers.
//!
//! Nothing here touches the UI or the disk: the editor runs [`agent::run_turn`] on a thread with
//! a copy of the project and applies the edited project it sends back.

pub mod activity;
pub mod agent;
pub mod client;
pub mod host;
mod items;
mod layers;
pub mod motion;
pub mod tools;

pub use agent::{run_turn, Conversation, Event};
pub use client::{Route, Settings, Usage, MODELS};
pub use host::{Action, Host};
pub use tools::Workspace;
