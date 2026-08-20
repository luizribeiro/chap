//! Guest authoring facade for SAGE plugins.
//!
//! The shared `sage:agent` WIT package lives in this crate's `wit` directory.
//! Built-in plugins reference it as `../../crates/sage-plugin/wit` and select
//! either the `provider-plugin` or `tool-plugin` world.

mod cap;
mod export;
#[path = "http.rs"]
mod http_reexport;
pub mod types;

#[doc(hidden)]
pub use lockgate_plugin::{__private, __wit_bindgen, alloc};
pub use lockgate_plugin::{
    Deserialize, HttpOrigin, JsonSchema, MetadataSource, Need, Needs, NoSettings, Permission,
    Plugin, ScopeRef, ScopedPermission, SettingsPolicy, export, generate, schemars, serde,
};

pub use cap::net;
pub use export::{provider, tools};
pub use http_reexport::http;
