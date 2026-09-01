lockgate::host_bindings!({
    path: "../chap-wit/wit",
    world: "host",
    imports: [
        { interface: "exec", feature: "exec" },
        { interface: "state", feature: "state" },
        { interface: "vm", feature: "vm" },
    ],
    imports_type: CapabilityHost,
    data: (),
});

#[derive(Clone)]
pub(super) struct CapabilityHost {
    #[cfg(feature = "exec")]
    pub(crate) executor: std::sync::Arc<chap_exec::host::Executor>,
    #[cfg(feature = "state")]
    pub(crate) store: chap_state::host::StateStore,
    #[cfg(feature = "vm")]
    pub(crate) vm: vm_host::VmHost,
}

#[cfg(feature = "exec")]
pub(super) mod exec_host;
#[cfg(feature = "state")]
pub(super) mod state_host;
#[cfg(feature = "vm")]
pub(super) mod vm_host;
