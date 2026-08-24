//! Guest authoring facade for CHAP plugins.
//!
//! The shared `chap:agent` WIT package lives in this crate's `wit` directory.
//! Built-in plugins reference it as `../../crates/chap-plugin/wit` and select
//! either the `provider-plugin` or `tool-plugin` world.

mod plugin;
mod roles;
#[doc(hidden)]
pub mod types;

pub use chap_plugin_macros::plugin;
#[cfg(feature = "http")]
pub use lockgate_http as http;
#[doc(hidden)]
pub use lockgate_plugin::{self as __lockgate, __private, __wit_bindgen, alloc, generate};
pub use lockgate_plugin::{
    EnvVarName, HttpOrigin, MetadataSource, Need, Needs, NoSettings, Permission, ScopeRef,
    ScopedPermission, SettingsPolicy, env, net,
};

pub use plugin::Plugin;
pub use roles::provider;
pub use roles::tools;
