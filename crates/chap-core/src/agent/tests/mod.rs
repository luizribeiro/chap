mod admission;
mod agent_loop;
mod context;
mod fixtures;

fn load_test_builder(config_path: &std::path::Path) -> super::AgentBuilder {
    super::AgentBuilder::load(config_path)
        .unwrap()
        .state_dir(config_path.parent().unwrap())
}
