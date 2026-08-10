use crate::tui::model::{ToolMessage, ToolState};
use iocraft::prelude::*;
use serde_json::Value;
use std::sync::Arc;

const MAX_ARGUMENT_SUMMARY_CHARS: usize = 160;
const MAX_ARGUMENT_VALUE_CHARS: usize = 80;

#[derive(Default, Props)]
pub struct ToolViewProps {
    pub tool: ToolMessage,
}

#[component]
pub fn ToolView(mut hooks: Hooks, props: &ToolViewProps) -> impl Into<AnyElement<'static>> {
    let mut expanded = hooks.use_state(|| false);
    let (symbol, status, color) = match &props.tool.state {
        ToolState::Requested => ("○", "requested", Color::DarkGrey),
        ToolState::Running => ("●", "running", Color::Yellow),
        ToolState::Interrupted => ("–", "interrupted", Color::DarkGrey),
        ToolState::Finished(Ok(_)) => ("✓", "done", Color::Green),
        ToolState::Finished(Err(_)) => ("×", "failed", Color::Red),
    };
    let name = humanize_name(&props.tool.name);
    let summary = summarize_arguments(&props.tool.arguments);
    let has_details = !props.tool.arguments.trim().is_empty()
        || matches!(props.tool.state, ToolState::Finished(Ok(_)));
    let details_label = if expanded.get() {
        "[hide]"
    } else {
        "[details]"
    };
    let error = match &props.tool.state {
        ToolState::Finished(Err(error)) => Some(Arc::clone(error)),
        _ => None,
    };
    let details = if expanded.get() {
        Some(element! {
            ToolDetails(tool: props.tool.clone())
        })
    } else {
        None
    };

    element! {
        View(
            width: 100pct,
            flex_direction: FlexDirection::Column,
            padding_left: 1,
        ) {
            View(width: 100pct, flex_direction: FlexDirection::Row) {
                Text(content: format!("{symbol} "), color)
                Text(content: name, weight: Weight::Bold)
                Text(content: format!(" · {status}"), color: Color::DarkGrey)
                View(flex_grow: 1.0_f32)
                #(if has_details {
                    Some(element! {
                        Button(handler: move |_| expanded.set(!expanded.get())) {
                            Text(content: details_label, color: Color::DarkGrey)
                        }
                    })
                } else {
                    None
                })
            }
            #(summary.map(|summary| element! {
                View(padding_left: 2) {
                    Text(content: summary, color: Color::DarkGrey)
                }
            }))
            #(error.map(|error| element! {
                View(padding_left: 2) {
                    Text(content: error.to_string(), color: Color::Red)
                }
            }))
            #(details)
        }
    }
}

#[derive(Default, Props)]
struct ToolDetailsProps {
    tool: ToolMessage,
}

#[component]
fn ToolDetails(props: &ToolDetailsProps) -> impl Into<AnyElement<'static>> {
    let output = match &props.tool.state {
        ToolState::Finished(Ok(output)) => Some(Arc::clone(output)),
        _ => None,
    };

    element! {
        View(
            flex_direction: FlexDirection::Column,
            padding_left: 2,
            padding_top: 1,
        ) {
            #(if props.tool.arguments.trim().is_empty() {
                None
            } else {
                Some(element! {
                    View(flex_direction: FlexDirection::Column) {
                        Text(content: "input", color: Color::DarkGrey, weight: Weight::Bold)
                        Text(content: props.tool.arguments.to_string())
                    }
                })
            })
            #(output.map(|output| element! {
                View(flex_direction: FlexDirection::Column, padding_top: 1) {
                    Text(content: "output", color: Color::DarkGrey, weight: Weight::Bold)
                    Text(content: output.to_string())
                }
            }))
        }
    }
}

fn humanize_name(name: &str) -> String {
    name.split(['_', '-'])
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn summarize_arguments(arguments: &str) -> Option<String> {
    let arguments = arguments.trim();
    if arguments.is_empty() || arguments == "{}" {
        return None;
    }

    let summary = match serde_json::from_str::<Value>(arguments) {
        Ok(Value::Object(fields)) => fields
            .iter()
            .map(|(name, value)| format!("{name}: {}", summarize_value(value)))
            .collect::<Vec<_>>()
            .join(" · "),
        Ok(value) => summarize_value(&value),
        Err(_) => arguments.split_whitespace().collect::<Vec<_>>().join(" "),
    };

    (!summary.is_empty()).then(|| truncate(&summary, MAX_ARGUMENT_SUMMARY_CHARS))
}

fn summarize_value(value: &Value) -> String {
    match value {
        Value::String(value) => format!("“{}”", truncate(value, MAX_ARGUMENT_VALUE_CHARS)),
        Value::Array(values) => format!(
            "{} {}",
            values.len(),
            if values.len() == 1 { "item" } else { "items" }
        ),
        Value::Object(_) => "{…}".to_owned(),
        Value::Null | Value::Bool(_) | Value::Number(_) => value.to_string(),
    }
}

fn truncate(value: &str, max_chars: usize) -> String {
    let mut characters = value.chars();
    let truncated = characters.by_ref().take(max_chars).collect::<String>();
    if characters.next().is_some() {
        format!("{truncated}…")
    } else {
        truncated
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn humanizes_tool_names_without_tool_specific_rules() {
        assert_eq!(humanize_name("web_search"), "web search");
        assert_eq!(humanize_name("read-file"), "read file");
    }

    #[test]
    fn summarizes_json_arguments_generically() {
        assert_eq!(
            summarize_arguments(r#"{"query":"iocraft markdown","limit":10,"urls":["a","b"]}"#),
            Some("limit: 10 · query: “iocraft markdown” · urls: 2 items".to_owned())
        );
    }

    #[test]
    fn falls_back_to_compact_plain_text_arguments() {
        assert_eq!(
            summarize_arguments("some   unstructured\ninput"),
            Some("some unstructured input".to_owned())
        );
        assert_eq!(summarize_arguments("{}"), None);
    }
}
