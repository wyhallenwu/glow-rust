//! Core library for the Rust edition of Glow.

pub mod cli;
pub mod config;
pub mod discover;
pub mod document;
pub mod render;
pub mod source;
pub mod tui;
pub mod tunnel;
pub mod web;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
