use iocraft::prelude::*;

#[derive(Default, Props)]
pub struct HeaderProps {
    pub provider: String,
}

#[component]
pub fn Header(props: &HeaderProps) -> impl Into<AnyElement<'static>> {
    element! {
        View(
            width: 100pct,
            padding_left: 1,
            padding_right: 1,
            margin_bottom: 1,
            flex_direction: FlexDirection::Row,
            justify_content: JustifyContent::SpaceBetween,
        ) {
            Text(content: "CHAP", color: Color::Cyan, weight: Weight::Bold)
            Text(
                content: format!("provider: {}", props.provider),
                color: Color::DarkGrey,
            )
        }
    }
}
