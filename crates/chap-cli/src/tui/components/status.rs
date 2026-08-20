use iocraft::prelude::*;

#[derive(Default, Props)]
pub struct StatusViewProps {
    pub content: String,
}

#[component]
pub fn StatusView(props: &StatusViewProps) -> impl Into<AnyElement<'static>> {
    element! {
        Text(content: props.content.clone(), color: Color::DarkGrey)
    }
}
