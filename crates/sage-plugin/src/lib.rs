//! Guest authoring facade for SAGE plugins.
//!
//! The shared `sage:agent` WIT package lives in this crate's `wit` directory.
//! Built-in plugins reference it as `../../crates/sage-plugin/wit` and select
//! either the `provider-plugin` or `tool-plugin` world.

mod cap;
mod export;
#[cfg(feature = "http")]
#[path = "http.rs"]
mod http_reexport;
mod plugin;
#[cfg(any(feature = "provider", feature = "tools"))]
pub mod types;

#[doc(hidden)]
pub use lockgate_plugin::{self as __lockgate, __private, __wit_bindgen, alloc};
pub use lockgate_plugin::{
    Deserialize, HttpOrigin, JsonSchema, MetadataSource, Need, Needs, NoSettings, Permission,
    ScopeRef, ScopedPermission, SettingsPolicy, export, generate, schemars, serde,
};
pub use sage_plugin_macros::Settings;

pub use cap::net;
#[cfg(feature = "provider")]
pub use export::{Provider, provider};
#[cfg(feature = "tools")]
pub use export::{Tools, tools};
#[cfg(feature = "http")]
pub use http_reexport::http;
pub use plugin::Plugin;
