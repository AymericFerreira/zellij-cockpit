//! Shared library for zellij-cockpit.
//!
//! `types` is compiled for both the WASM plugin and the native helper (it is the
//! JSON contract piped between them). Everything else is native-only: it touches
//! syscalls / the filesystem and never compiles into the WASM sandbox.

pub mod types;

pub mod config;

/// The tab strip and the running-command marker. Native so it can be tested
/// without a WASM host; the plugin feeds it plain data.
pub mod bar;

/// Which panes are running a foreground shell command.
pub mod activity;

#[cfg(feature = "native")]
pub mod usage;

#[cfg(feature = "native")]
pub mod system;

#[cfg(feature = "native")]
pub mod claude;

#[cfg(feature = "native")]
pub mod codex;
