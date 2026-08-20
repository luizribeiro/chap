//! Shared data types from the `sage:agent/types` interface.

mod bindings {
    use lockgate_plugin::__wit_bindgen as wit_bindgen;

    lockgate_plugin::__wit_bindgen::generate!({
        path: "wit",
        world: "sdk-types",
        generate_all,
    });
}

pub use bindings::sage::agent::types::*;
