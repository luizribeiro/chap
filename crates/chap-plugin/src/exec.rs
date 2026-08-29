//! Process execution types and functions from the `chap:agent/exec` interface.

mod bindings {
    lockgate_plugin::__wit_bindgen::generate!({
        path: "../chap-wit/wit",
        world: "sdk-exec",
        runtime_path: "lockgate_plugin::__wit_bindgen::rt",
    });
}

pub use bindings::chap::agent::exec::*;
