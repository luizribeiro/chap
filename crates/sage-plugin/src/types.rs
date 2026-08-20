//! Shared data types from the `sage:agent/types` interface.

mod bindings {
    use lockgate_plugin::__wit_bindgen as wit_bindgen;

    lockgate_plugin::__wit_bindgen::generate!({
        path: "wit",
        world: "sdk-types",
    });
}

pub use bindings::sage::agent::types::*;
