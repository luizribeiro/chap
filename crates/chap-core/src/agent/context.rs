use super::{AgentInner, CONTEXT_ASSEMBLY_DEADLINE, PLUGIN_FUEL_PER_CALL, bindings};
use crate::SessionId;
use bindings::context as context_bindings;
use futures::future::join_all;
use lockgate::{CallError, InvocationCtx};
use std::{collections::BTreeMap, future::Future, pin::Pin};

pub(super) type ContextFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Option<String>, String>> + Send + 'a>>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ContextSegment {
    pub(super) id: String,
    pub(super) content: String,
    pub(super) priority: i32,
}

type LastGood = BTreeMap<String, Vec<ContextSegment>>;
type PluginResults = BTreeMap<String, Result<Vec<ContextSegment>, String>>;

#[derive(Debug, Eq, PartialEq)]
struct ContextAssembly {
    block: Result<Option<String>, String>,
    last_good: LastGood,
    warnings: Vec<(String, String)>,
}

impl AgentInner {
    pub(super) async fn assemble_context(
        &self,
        session: SessionId,
    ) -> Result<Option<String>, String> {
        let calls = self
            .plugins
            .iter()
            .filter(|(_, plugin)| plugin.has_role(&chap_wit::CONTEXT))
            .map(|(id, plugin)| async move {
                (
                    id.clone(),
                    self.request_context_segments(&plugin.handle).await,
                )
            });
        let results = join_all(calls).await.into_iter().collect();

        let mut last_good = self.context_last_good.lock().await;
        let prior = last_good
            .iter()
            .filter(|((stored_session, _), _)| *stored_session == session)
            .map(|((_, plugin), segments)| (plugin.clone(), segments.clone()))
            .collect();
        let assembly = compose_context(results, prior);
        last_good.retain(|(stored_session, _), _| *stored_session != session);
        for (plugin, segments) in assembly.last_good {
            last_good.insert((session, plugin), segments);
        }
        drop(last_good);

        // TODO: chap has no diagnostics channel, so this writes to stderr and interleaves
        // with the TUI's inline render. Degraded-but-working plugin state has nowhere else
        // to go today: the session event kinds are all run/tool/steering lifecycle, and
        // guests cannot report it either — Lockgate builds their WASI context without
        // inheriting stdio, so a plugin's own output is discarded.
        //
        // When a channel exists, `compose_context` should report through it directly and
        // `ContextAssembly::warnings` should be deleted along with this loop. That field
        // exists only because a pure function had no way to emit anything.
        for (plugin, error) in assembly.warnings {
            eprintln!(
                "Warning: context plugin `{plugin}` failed; reusing its last-good segments: {error}"
            );
        }
        assembly.block
    }

    async fn request_context_segments(
        &self,
        plugin: &lockgate::PluginHandle,
    ) -> Result<Vec<ContextSegment>, String> {
        self.lockgate
            .client::<context_bindings::Role>(plugin)
            .map_err(|error| error.to_string())?
            .segments(InvocationCtx::bounded_with_deadline(
                PLUGIN_FUEL_PER_CALL,
                CONTEXT_ASSEMBLY_DEADLINE,
            ))
            .await
            .map_err(context_call_error)?
            .map_err(|error| error.to_string())
            .map(|segments| segments.into_iter().map(Into::into).collect())
    }
}

fn compose_context(results: PluginResults, mut last_good: LastGood) -> ContextAssembly {
    let mut ordered = Vec::new();
    let mut warnings = Vec::new();
    let mut failure = None;

    for (plugin, result) in results {
        let segments = match result {
            Ok(segments) if segments.is_empty() => {
                last_good.remove(&plugin);
                Vec::new()
            }
            Ok(segments) => {
                last_good.insert(plugin.clone(), segments.clone());
                segments
            }
            Err(error) => match last_good.get(&plugin) {
                Some(segments) => {
                    warnings.push((plugin.clone(), error));
                    segments.clone()
                }
                None => {
                    failure.get_or_insert_with(|| {
                        format!("context plugin `{plugin}` failed: {error}")
                    });
                    Vec::new()
                }
            },
        };

        ordered.extend(
            segments
                .into_iter()
                .enumerate()
                .map(|(index, segment)| (segment.priority, plugin.clone(), index, segment.content)),
        );
    }

    ordered.sort_by(|left, right| (&left.0, &left.1, &left.2).cmp(&(&right.0, &right.1, &right.2)));
    let block = match failure {
        Some(error) => Err(error),
        None if ordered.is_empty() => Ok(None),
        None => Ok(Some(
            ordered
                .into_iter()
                .map(|(_, _, _, content)| content)
                .collect::<Vec<_>>()
                .join("\n\n"),
        )),
    };

    ContextAssembly {
        block,
        last_good,
        warnings,
    }
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
        let assembly = compose_context(
            BTreeMap::from([
                ("first-plugin".to_owned(), Ok(vec![segment("late", 20)])),
                ("second-plugin".to_owned(), Ok(vec![segment("early", -10)])),
            ]),
            BTreeMap::new(),
        );

        assert_eq!(assembly.block, Ok(Some("early\n\nlate".to_owned())));
    }

    #[test]
    fn ties_break_by_plugin_id_then_segment_index() {
        let assembly = compose_context(
            BTreeMap::from([
                (
                    "zeta".to_owned(),
                    Ok(vec![segment("zeta-1", 0), segment("zeta-2", 0)]),
                ),
                (
                    "alpha".to_owned(),
                    Ok(vec![segment("alpha-1", 0), segment("alpha-2", 0)]),
                ),
            ]),
            BTreeMap::new(),
        );

        assert_eq!(
            assembly.block,
            Ok(Some("alpha-1\n\nalpha-2\n\nzeta-1\n\nzeta-2".to_owned()))
        );
    }

    #[test]
    fn empty_success_clears_last_good_and_contributes_nothing() {
        let assembly = compose_context(
            BTreeMap::from([("example".to_owned(), Ok(Vec::new()))]),
            BTreeMap::from([("example".to_owned(), vec![segment("stale", 0)])]),
        );

        assert_eq!(assembly.block, Ok(None));
        assert!(assembly.last_good.is_empty());
    }

    #[test]
    fn failure_after_success_reuses_last_good() {
        let successful = compose_context(
            BTreeMap::from([("example".to_owned(), Ok(vec![segment("stable", 0)]))]),
            BTreeMap::new(),
        );
        let failed = compose_context(
            BTreeMap::from([("example".to_owned(), Err("unavailable".to_owned()))]),
            successful.last_good,
        );

        assert_eq!(failed.block, Ok(Some("stable".to_owned())));
        assert_eq!(
            failed.warnings,
            [("example".to_owned(), "unavailable".to_owned())]
        );
    }

    #[test]
    fn first_failure_names_the_plugin() {
        let assembly = compose_context(
            BTreeMap::from([("broken-context".to_owned(), Err("unavailable".to_owned()))]),
            BTreeMap::new(),
        );

        assert_eq!(
            assembly.block,
            Err("context plugin `broken-context` failed: unavailable".to_owned())
        );
    }

    fn segment(content: &str, priority: i32) -> ContextSegment {
        ContextSegment {
            id: content.to_owned(),
            content: content.to_owned(),
            priority,
        }
    }
}
