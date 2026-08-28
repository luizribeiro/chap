use super::{AgentInner, PluginCall, bindings};
use crate::{config::roles::ContextChannel, session::AssembledContext};
use bindings::context as context_bindings;
use futures::future::join_all;
use lockgate::CallError;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ContextSegment {
    pub(super) id: String,
    pub(super) content: String,
    pub(super) priority: i32,
}

type PluginResults = BTreeMap<String, PluginResult>;

struct PluginResult {
    channel: ContextChannel,
    segments: Result<Vec<ContextSegment>, String>,
}

impl AgentInner {
    pub(super) async fn assemble_context(&self) -> Result<AssembledContext, String> {
        let calls = self
            .plugins
            .iter()
            .filter(|(_, plugin)| plugin.has_role(&chap_wit::CONTEXT))
            .map(|(id, plugin)| async move {
                (
                    id.clone(),
                    PluginResult {
                        channel: plugin.role_settings.context().channel(),
                        segments: self.request_context_segments(&plugin.handle).await,
                    },
                )
            });
        let results = join_all(calls).await.into_iter().collect();
        compose_context(results)
    }

    async fn request_context_segments(
        &self,
        plugin: &lockgate::PluginHandle,
    ) -> Result<Vec<ContextSegment>, String> {
        self.lockgate
            .client::<context_bindings::Role>(plugin)
            .map_err(|error| error.to_string())?
            .segments(
                self.call_budgets
                    .resolve(PluginCall::ContextSegments)
                    .invocation_context(),
            )
            .await
            .map_err(context_call_error)?
            .map_err(|error| error.to_string())
            .map(|segments| segments.into_iter().map(Into::into).collect())
    }
}

fn compose_context(results: PluginResults) -> Result<AssembledContext, String> {
    let mut system = Vec::new();
    let mut context = Vec::new();

    for (plugin, result) in results {
        let segments = result
            .segments
            .map_err(|error| format!("context plugin `{plugin}` failed: {error}"))?;
        let ordered = match result.channel {
            ContextChannel::Context => &mut context,
            ContextChannel::System => &mut system,
        };
        ordered.extend(
            segments
                .into_iter()
                .enumerate()
                .map(|(index, segment)| (segment.priority, plugin.clone(), index, segment.content)),
        );
    }

    Ok(AssembledContext {
        system: render_channel(system),
        context: render_channel(context),
    })
}

fn render_channel(mut segments: Vec<(i32, String, usize, String)>) -> Option<String> {
    segments
        .sort_by(|left, right| (&left.0, &left.1, &left.2).cmp(&(&right.0, &right.1, &right.2)));
    (!segments.is_empty()).then(|| {
        segments
            .into_iter()
            .map(|(_, _, _, content)| content)
            .collect::<Vec<_>>()
            .join("\n\n")
    })
}

fn context_call_error(error: CallError) -> String {
    match error {
        CallError::DeadlineExceeded { deadline } => {
            format!("timed out after {deadline:?}")
        }
        error => error.to_string(),
    }
}

impl From<context_bindings::Segment> for ContextSegment {
    fn from(segment: context_bindings::Segment) -> Self {
        Self {
            id: segment.id,
            content: segment.content,
            priority: segment.priority,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn composes_segments_by_priority() {
        let assembly = compose_context(BTreeMap::from([
            (
                "first-plugin".to_owned(),
                success(ContextChannel::Context, vec![segment("late", 20)]),
            ),
            (
                "second-plugin".to_owned(),
                success(ContextChannel::Context, vec![segment("early", -10)]),
            ),
        ]));

        assert_eq!(
            assembly,
            Ok(AssembledContext {
                system: None,
                context: Some("early\n\nlate".to_owned()),
            })
        );
    }

    #[test]
    fn ties_break_by_plugin_id_then_segment_index() {
        let assembly = compose_context(BTreeMap::from([
            (
                "zeta".to_owned(),
                success(
                    ContextChannel::Context,
                    vec![segment("zeta-1", 0), segment("zeta-2", 0)],
                ),
            ),
            (
                "alpha".to_owned(),
                success(
                    ContextChannel::Context,
                    vec![segment("alpha-1", 0), segment("alpha-2", 0)],
                ),
            ),
        ]));

        assert_eq!(
            assembly.unwrap().context,
            Some("alpha-1\n\nalpha-2\n\nzeta-1\n\nzeta-2".to_owned())
        );
    }

    #[test]
    fn priorities_sort_independently_within_each_channel() {
        let assembly = compose_context(BTreeMap::from([
            (
                "system-late".to_owned(),
                success(ContextChannel::System, vec![segment("system late", 50)]),
            ),
            (
                "system-early".to_owned(),
                success(ContextChannel::System, vec![segment("system early", -5)]),
            ),
            (
                "context-late".to_owned(),
                success(ContextChannel::Context, vec![segment("context late", 100)]),
            ),
            (
                "context-early".to_owned(),
                success(
                    ContextChannel::Context,
                    vec![segment("context early", -100)],
                ),
            ),
        ]));

        assert_eq!(
            assembly,
            Ok(AssembledContext {
                system: Some("system early\n\nsystem late".to_owned()),
                context: Some("context early\n\ncontext late".to_owned()),
            })
        );
    }

    #[test]
    fn empty_channel_produces_no_message() {
        let assembly = compose_context(BTreeMap::from([
            (
                "empty-context".to_owned(),
                success(ContextChannel::Context, Vec::new()),
            ),
            (
                "system".to_owned(),
                success(ContextChannel::System, vec![segment("operator", 0)]),
            ),
        ]));

        assert_eq!(
            assembly,
            Ok(AssembledContext {
                system: Some("operator".to_owned()),
                context: None,
            })
        );
    }

    #[test]
    fn failure_names_the_plugin() {
        let assembly = compose_context(BTreeMap::from([(
            "broken-context".to_owned(),
            failure(ContextChannel::Context, "unavailable"),
        )]));

        assert_eq!(
            assembly,
            Err("context plugin `broken-context` failed: unavailable".to_owned())
        );
    }

    fn success(channel: ContextChannel, segments: Vec<ContextSegment>) -> PluginResult {
        PluginResult {
            channel,
            segments: Ok(segments),
        }
    }

    fn failure(channel: ContextChannel, error: &str) -> PluginResult {
        PluginResult {
            channel,
            segments: Err(error.to_owned()),
        }
    }

    fn segment(content: &str, priority: i32) -> ContextSegment {
        ContextSegment {
            id: content.to_owned(),
            content: content.to_owned(),
            priority,
        }
    }
}
