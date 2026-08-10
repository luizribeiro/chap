use crate::tui::model::PendingSteering;
use iocraft::prelude::*;
use sage_core::SteeringId;

#[derive(Default, Props)]
pub struct PendingSteeringViewProps {
    pub steering: Option<PendingSteering>,
    pub on_discard: Handler<SteeringId>,
}

#[component]
pub fn PendingSteeringView(props: &PendingSteeringViewProps) -> impl Into<AnyElement<'static>> {
    let Some(steering) = &props.steering else {
        return element!(View).into_any();
    };

    element! {
        View(width: 100pct, flex_direction: FlexDirection::Column) {
            View(width: 100pct, flex_direction: FlexDirection::Row) {
                Text(content: "you · queued", color: Color::DarkGrey, weight: Weight::Bold)
                View(flex_grow: 1.0_f32)
                Button(handler: props.on_discard.bind(steering.id)) {
                    Text(content: "[discard]", color: Color::DarkGrey)
                }
            }
            Text(content: steering.input.clone(), color: Color::DarkGrey)
        }
    }
    .into_any()
}
