#[cfg(not(feature = "exec"))]
#[cfg(not(feature = "state"))]
lockgate::host_bindings!({
    path: "../chap-wit/wit",
    world: "host",
});

#[cfg(feature = "exec")]
#[cfg(not(feature = "state"))]
lockgate::host_bindings!({
    path: "../chap-wit/wit",
    world: "host-exec",
    imports_type: CapabilityHost,
    data: (),
});

#[cfg(not(feature = "exec"))]
#[cfg(feature = "state")]
lockgate::host_bindings!({
    path: "../chap-wit/wit",
    world: "host-state",
    imports_type: CapabilityHost,
    data: (),
});

#[cfg(feature = "exec")]
#[cfg(feature = "state")]
lockgate::host_bindings!({
    path: "../chap-wit/wit",
    world: "host-exec-state",
    imports_type: CapabilityHost,
    data: (),
});

#[derive(Clone)]
#[cfg_attr(not(feature = "exec"), allow(dead_code))]
pub(super) struct CapabilityHost {
    #[cfg(feature = "exec")]
    pub(crate) executor: std::sync::Arc<chap_exec::host::Executor>,
    #[cfg(feature = "state")]
    pub(crate) store: chap_state::host::StateStore,
}

#[cfg(feature = "exec")]
pub(super) mod exec_host;
#[cfg(feature = "state")]
pub(super) mod state_host;
