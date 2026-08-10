use iocraft::prelude::*;
use std::time::Duration;

const FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

#[component]
pub fn Spinner(mut hooks: Hooks) -> impl Into<AnyElement<'static>> {
    let mut frame = hooks.use_state(|| 0usize);

    hooks.use_future(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(80)).await;
            frame.set((frame.get() + 1) % FRAMES.len());
        }
    });

    element! {
        Text(
            content: format!("{} thinking…", FRAMES[frame.get()]),
            color: Color::Yellow,
        )
    }
}
