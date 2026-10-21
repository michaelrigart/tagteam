//! tagteam's terminal UI (TUI spec): an Elm-style state machine drawn with ratatui, worker
//! lanes over a read port, and the action port the `tagteam` binary implements. An internal
//! library with no stable API.
#![forbid(unsafe_code)]

pub(crate) mod actions;
mod ask;
pub(crate) mod daemon;
mod detect;
pub mod host;
mod input;
pub(crate) mod lanes;
pub mod live;
pub(crate) mod msg;
pub(crate) mod overlay;
mod runtime;
pub(crate) mod screens;
pub mod source;
pub(crate) mod state;
mod term;
mod theme;
pub(crate) mod update;
pub(crate) mod view;

pub use runtime::{TuiExit, TuiOptions, run};
pub use state::Screen;

#[cfg(test)]
pub(crate) mod testutil;
