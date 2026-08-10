use iocraft::prelude::*;

#[derive(Default, Props)]
pub struct PromptProps {
    pub busy: bool,
    pub value: String,
    pub on_change: HandlerMut<'static, String>,
}

#[component]
pub fn Prompt(props: &mut PromptProps) -> impl Into<AnyElement<'static>> {
    element! {
        View(
            width: 100pct,
            height: 3,
            padding_left: 1,
            padding_right: 1,
            border_style: BorderStyle::Single,
            border_edges: Edges::Top | Edges::Bottom,
            border_color: Color::DarkGrey,
            flex_direction: FlexDirection::Row,
        ) {
            Text(content: "› ", color: Color::Cyan, weight: Weight::Bold)
            View(flex_grow: 1.0_f32) {
                TextInput(
                    has_focus: !props.busy,
                    value: props.value.clone(),
                    on_change: props.on_change.take(),
                )
            }
        }
    }
}
