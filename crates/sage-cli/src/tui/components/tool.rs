use crate::tui::model::{ToolMessage, ToolState};
use iocraft::prelude::*;

#[derive(Default, Props)]
pub struct ToolViewProps {
    pub tool: ToolMessage,
}

#[component]
pub fn ToolView(props: &ToolViewProps) -> impl Into<AnyElement<'static>> {
    let (status, color, result) = match &props.tool.state {
        ToolState::Requested => ("requested", Color::DarkGrey, None),
        ToolState::Running => ("running", Color::DarkGrey, None),
        ToolState::Finished(Ok(output)) => ("done", Color::Green, Some(("output", output))),
        ToolState::Finished(Err(error)) => ("failed", Color::Red, Some(("error", error))),
    };
    let label = format!("tool · {} · {status}", props.tool.name);
    let mut details = Vec::new();
    if !props.tool.arguments.is_empty() {
        details.push(format!("input  {}", props.tool.arguments));
    }
    if let Some((label, content)) = result {
        details.push(format!("{label}  {content}"));
    }

    element! {
        View(flex_direction: FlexDirection::Column) {
            Text(content: label, color, weight: Weight::Bold)
            Text(content: details.join("\n"))
        }
    }
}
