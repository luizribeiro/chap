#![no_std]

//! Authority contract shared by state hosts and plugins.

extern crate alloc;

#[cfg(feature = "host")]
extern crate std;

#[cfg(feature = "host")]
pub mod host;

#[lockgate_policy::capability("state")]
pub mod state {
    use lockgate_policy::Permission;

    /// Read and write the plugin's host-owned key-value scratch store.
    pub const ACCESS: Permission = Permission::new("access");
}
