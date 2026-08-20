//! Minimal facade for implementing SAGE plugins on Lockgate.
//!
//! The shared `sage:agent` WIT package lives in this crate's `wit` directory.
//! Built-in plugins reference it as `../../crates/sage-plugin/wit` and select
//! either the `provider-plugin` or `tool-plugin` world.

pub use lockgate_plugin::*;
