use super::Spinner;
use crate::tui::model::UsageModel;
use iocraft::prelude::*;

#[derive(Default, Props)]
pub struct FooterProps {
    pub busy: bool,
    pub usage: Option<UsageModel>,
}

#[component]
pub fn Footer(props: &FooterProps) -> impl Into<AnyElement<'static>> {
    let controls = if props.busy {
        "enter steer  •  ctrl+g editor  •  esc interrupt  •  ctrl+d quit"
    } else {
        "enter send  •  ctrl+g editor  •  ctrl+d quit"
    };

    element! {
        View(
            width: 100pct,
            padding_left: 1,
            padding_right: 1,
            flex_direction: FlexDirection::Row,
            justify_content: JustifyContent::SpaceBetween,
        ) {
            View(flex_grow: 1.0_f32) {
                #(if props.busy { Some(element!(Spinner)) } else { None })
            }
            #(props.usage.map(|usage| element! {
                View(margin_right: 2) {
                    Text(
                        content: format_usage(usage),
                        color: Color::DarkGrey,
                    )
                }
            }))
            Text(content: controls, color: Color::DarkGrey)
        }
    }
}

fn format_usage(usage: UsageModel) -> String {
    // Cached input bills at a fraction of fresh input, so counting it at full weight
    // overstates the session precisely when caching is working.
    let billed_tokens = usage
        .session
        .input_tokens
        .saturating_sub(usage.session.cached_input_tokens.unwrap_or_default())
        .saturating_add(usage.session.output_tokens);
    let mut text = format!(
        "ctx {} · {} billed",
        abbreviate_tokens(usage.context_tokens),
        abbreviate_tokens(billed_tokens),
    );
    append_optional_counter(&mut text, usage.session.cached_input_tokens, "cached");
    append_optional_counter(&mut text, usage.session.reasoning_tokens, "reasoning");
    text
}

fn append_optional_counter(text: &mut String, tokens: Option<u64>, label: &str) {
    if let Some(tokens) = tokens.filter(|tokens| *tokens > 0) {
        text.push_str(&format!(" · {} {label}", abbreviate_tokens(tokens)));
    }
}

fn abbreviate_tokens(tokens: u64) -> String {
    match tokens {
        0..1_000 => tokens.to_string(),
        1_000..1_000_000 => format!("{:.1}k", tokens as f64 / 1_000.0),
        _ => format!("{:.1}M", tokens as f64 / 1_000_000.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abbreviates_token_counts() {
        for (tokens, expected) in [
            (999, "999"),
            (1_000, "1.0k"),
            (1_049, "1.0k"),
            (1_050, "1.1k"),
            (999_999, "1000.0k"),
            (1_000_000, "1.0M"),
        ] {
            assert_eq!(abbreviate_tokens(tokens), expected);
        }
    }
}
