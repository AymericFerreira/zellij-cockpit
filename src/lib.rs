//! Shared library for zellij-cockpit.
//!
//! `types` is compiled for both the WASM plugin and the native helper (it is the
//! JSON contract piped between them). `system` and `claude` are native-only:
//! they touch syscalls / the filesystem and never compile into the WASM sandbox.

pub mod types;

#[cfg(feature = "native")]
pub mod system;

#[cfg(feature = "native")]
pub mod claude;
