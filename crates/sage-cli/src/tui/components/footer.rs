use super::Spinner;
use iocraft::prelude::*;

#[derive(Default, Props)]
pub struct FooterProps {
    pub busy: bool,
}

#[component]
pub fn Footer(props: &FooterProps) -> impl Into<AnyElement<'static>> {
    let action = if props.busy { "steer" } else { "send" };

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
            Text(content: format!("enter {action}  •  ctrl+d quit"), color: Color::DarkGrey)
        }
    }
}
