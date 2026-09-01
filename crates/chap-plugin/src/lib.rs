//! Guest authoring facade for CHAP plugins.
//!
//! The shared `chap:agent` WIT package and plugin-world composition live in the
//! `chap-wit` crate.

#[cfg(feature = "exec")]
#[doc(hidden)]
pub mod exec;
mod plugin;
mod roles;
#[cfg(feature = "state")]
pub mod state;
#[doc(hidden)]
pub mod types;

pub use chap_plugin_macros::plugin;
#[cfg(feature = "http")]
pub use lockgate_http as http;
#[doc(hidden)]
pub use lockgate_plugin::{self as __lockgate, __private, __wit_bindgen, alloc, generate};
pub use lockgate_plugin::{
    EnvVarName, HttpOrigin, MetadataSource, Need, Needs, NoSettings, Permission, ScopeRef,
    ScopedPermission, SettingsPolicy,
};

/// Capability permissions a plugin can request through [`Plugin::NEEDS`].
pub mod capabilities {
    #[cfg(feature = "exec")]
    pub use chap_exec::exec;
    #[cfg(feature = "state")]
    pub use chap_state::state;
    pub use lockgate_plugin::{env, net};
}

pub use plugin::Plugin;
pub use roles::context;
pub use roles::provider;
pub use roles::tools;
