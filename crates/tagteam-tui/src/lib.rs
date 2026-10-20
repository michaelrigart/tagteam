//! tagteam's terminal UI (TUI spec): an Elm-style state machine drawn with ratatui, worker
//! lanes over a read port, and the action port the `tagteam` binary implements. An internal
//! library with no stable API.
#![forbid(unsafe_code)]

pub mod actions;
mod ask;
pub mod daemon;
pub mod host;
mod input;
pub mod lanes;
pub mod live;
pub mod msg;
pub mod overlay;
mod runtime;
pub mod screens;
pub mod source;
pub mod state;
mod term;
mod theme;
pub mod update;
pub mod view;

pub use runtime::{TuiExit, TuiOptions, run};
pub use state::Screen;

#[cfg(test)]
pub(crate) mod testutil;
