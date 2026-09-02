use super::super::NO_PLUGINS_CONFIGURED;
use crate::render::{render_consent_error, render_load_error};
use chap_core::{
    AgentBuilder, DriftChange, DriftKind, ExportDrift, ExportDriftKind, PluginConsentReview,
};
use std::collections::BTreeSet;

pub(super) async fn grants_review(
    builder: &AgentBuilder,
    instance_id: Option<&str>,
) -> Result<String, String> {
    let consent_path = builder.consent_path().map_err(render_load_error)?;
    let mut output = format!("Consent store: {}\n", consent_path.display());
    let ids = match instance_id {
        Some(id) => vec![id.to_owned()],
        None => builder
            .plugins()
            .map(|(id, _)| id.to_owned())
            .collect::<Vec<_>>(),
    };
    if ids.is_empty() {
        output.push_str(NO_PLUGINS_CONFIGURED);
        return Ok(output);
    }

    output.push_str(
        "Concrete scopes come from chap.json; approval grants this exact resolved manifest.\n",
    );
    let review_ids = ids.iter().map(String::as_str).collect::<Vec<_>>();
    let reviews = builder
        .review_plugins(&review_ids)
        .await
        .map_err(render_consent_error)?;
    for (index, review) in reviews.iter().enumerate() {
        if index > 0 {
            output.push('\n');
        }
        output.push_str(&render_grant_review(review));
    }
    Ok(output)
}

fn render_grant_review(review: &PluginConsentReview) -> String {
    let mut output = format!(
        "Instance: {}\nPlugin: {}\n",
        review.manifest.plugin_id, review.manifest.plugin_label
    );
    match (&review.prior, &review.drift) {
        (None, _) => output.push_str("Status: NEEDS APPROVAL (first run)\n"),
        (Some(prior), None) => {
            output.push_str(&format!("Status: APPROVED at {}\n", prior.approved_at));
        }
        (Some(_), Some(drift)) if drift.blocks_admission => {
            output.push_str("Status: NEEDS APPROVAL (blocking permission expansion)\n");
        }
        (Some(_), Some(_)) => {
            output.push_str("Status: CHANGED (non-blocking; prior approval remains valid)\n");
        }
    }
    output.push_str("Exports:\n");
    if review.manifest.exported_interfaces.is_empty() {
        output.push_str("  (none)\n");
    }
    for exported_interface in &review.manifest.exported_interfaces {
        output.push_str(&format!("  - {exported_interface}\n"));
    }
    output.push_str("Grants:\n");
    if review.manifest.grants.is_empty() {
        output.push_str("  (none)\n");
    }
    for grant in &review.manifest.grants {
        output.push_str(&format!(
            "  - {}.{}\n    scopes: {}\n    optional: {}\n    reason: {}\n",
            grant.capability,
            grant.permission,
            render_scopes(&grant.scopes),
            if grant.optional { "yes" } else { "no" },
            grant.reason.as_deref().unwrap_or("(none)")
        ));
        if grant.capability == "exec" && grant.permission == "run" {
            output.push_str(
                "    warning: command prefixes bound entry points, not effects; run-helper programs grant arbitrary execution as the CHAP operator\n",
            );
        }
    }
    if let Some(drift) = &review.drift {
        output.push_str(if drift.blocks_admission {
            "Drift since approval (BLOCKING):\n"
        } else {
            "Drift since approval (non-blocking):\n"
        });
        for change in &drift.changes {
            output.push_str("  - ");
            output.push_str(&render_drift_change(change));
            output.push('\n');
        }
        for change in &drift.export_changes {
            output.push_str("  - ");
            output.push_str(&render_export_drift(change));
            output.push('\n');
        }
    }
    output
}

fn render_drift_change(change: &DriftChange) -> String {
    let permission = format!("{}.{}", change.capability, change.permission);
    match change.kind {
        DriftKind::NewGrant => format!(
            "now ALSO requests: {permission} → {} (NEW, BLOCKING)",
            render_optional_scopes(change.after.as_deref())
        ),
        DriftKind::ScopeWidened => {
            let before = change
                .before
                .as_deref()
                .unwrap_or_default()
                .iter()
                .collect::<BTreeSet<_>>();
            let added = change
                .after
                .as_deref()
                .unwrap_or_default()
                .iter()
                .filter(|scope| !before.contains(scope))
                .cloned()
                .collect::<Vec<_>>();
            format!(
                "now ALSO requests: {permission} → {} (WIDENED, BLOCKING)",
                render_scopes(&added)
            )
        }
        DriftKind::BecameRequired => {
            format!("now requires: {permission} (WAS OPTIONAL, BLOCKING)")
        }
        DriftKind::RemovedGrant => format!("no longer requests: {permission} (REMOVED)"),
        DriftKind::ScopeNarrowed => format!(
            "now requests fewer scopes: {permission} → {} (NARROWED)",
            render_optional_scopes(change.after.as_deref())
        ),
        DriftKind::BecameOptional => format!("now treats as optional: {permission} (OPTIONAL)"),
    }
}

fn render_export_drift(change: &ExportDrift) -> String {
    match change.kind {
        ExportDriftKind::Gained => format!(
            "now ALSO exports: {} (NEW ROLE, BLOCKING)",
            change.after.join(", ")
        ),
        ExportDriftKind::Lost => {
            format!("no longer exports: {} (REMOVED)", change.before.join(", "))
        }
        ExportDriftKind::VersionChanged => format!(
            "exports {} at {} (was {})",
            change.name,
            change.after.join(", "),
            change.before.join(", ")
        ),
    }
}

fn render_optional_scopes(scopes: Option<&[String]>) -> String {
    scopes
        .map(render_scopes)
        .unwrap_or_else(|| "(none)".to_owned())
}

fn render_scopes(scopes: &[String]) -> String {
    if scopes.is_empty() {
        "(unscoped)".to_owned()
    } else {
        scopes.join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chap_core::{
        ConsentManifest, ConsentRecord, DriftReport, GrantReview, PluginConsentReview,
    };

    fn consent_record(instance_id: &str, digest_byte: char) -> ConsentRecord {
        serde_json::from_value(serde_json::json!({
            "instance_id": instance_id,
            "fingerprint": format!("sha256:{}", digest_byte.to_string().repeat(64)),
            "grants": [{
                "capability": "net",
                "permission": "egress",
                "scopes": ["https://api.example.com"],
                "optional": false,
                "reason": "Call the configured API"
            }],
            "approved_at": "2026-08-19T14:30:00Z"
        }))
        .unwrap()
    }

    fn manifest(scopes: &[&str], digest_byte: char) -> ConsentManifest {
        let fingerprint = consent_record("example", digest_byte).request_digest;
        ConsentManifest {
            plugin_id: "example".into(),
            plugin_label: "Example provider".to_owned(),
            request_digest: fingerprint,
            component_digest: format!("sha256:{}", digest_byte.to_string().repeat(64)),
            exported_interfaces: vec!["chap:agent/provider@0.2.0".to_owned()],
            grants: vec![GrantReview {
                capability: "net".to_owned(),
                permission: "egress".to_owned(),
                scopes: scopes.iter().map(|scope| (*scope).to_owned()).collect(),
                optional: false,
                reason: Some("Call the configured API".to_owned()),
            }],
        }
    }

    #[tokio::test]
    async fn grants_review_starts_with_the_consent_store_path() {
        let directory = tempfile::tempdir().unwrap();
        let config_path = directory.path().join("chap.json");
        std::fs::write(&config_path, "{}").unwrap();
        let state_dir = directory.path().join("state");
        let builder = AgentBuilder::load(&config_path)
            .unwrap()
            .state_dir(&state_dir);

        let output = grants_review(&builder, None).await.unwrap();

        assert!(
            output.starts_with(&format!(
                "Consent store: {}\n",
                state_dir.join("consent.json").display()
            )),
            "{output}"
        );
    }

    #[test]
    fn renders_every_manifest_field_for_review() {
        let output = render_grant_review(&PluginConsentReview {
            manifest: manifest(&["https://api.example.com"], '1'),
            prior: None,
            drift: None,
        });

        assert!(output.contains("Instance: example"), "{output}");
        assert!(output.contains("Plugin: Example provider"), "{output}");
        assert!(output.contains("NEEDS APPROVAL (first run)"), "{output}");
        assert!(
            output.contains("Exports:\n  - chap:agent/provider@0.2.0\nGrants:"),
            "{output}"
        );
        assert!(output.contains("net.egress"), "{output}");
        assert!(
            output.contains("scopes: https://api.example.com"),
            "{output}"
        );
        assert!(output.contains("optional: no"), "{output}");
        assert!(
            output.contains("reason: Call the configured API"),
            "{output}"
        );
    }

    #[test]
    fn warns_that_exec_prefixes_can_grant_arbitrary_execution() {
        let mut manifest = manifest(&["cargo", "git commit"], '1');
        manifest.grants[0].capability = "exec".to_owned();
        manifest.grants[0].permission = "run".to_owned();

        let output = render_grant_review(&PluginConsentReview {
            manifest,
            prior: None,
            drift: None,
        });

        assert!(
            output.contains(
                "warning: command prefixes bound entry points, not effects; run-helper programs grant arbitrary execution as the CHAP operator"
            ),
            "{output}"
        );
    }

    #[test]
    fn renders_an_empty_exports_section() {
        let mut manifest = manifest(&["https://api.example.com"], '1');
        manifest.exported_interfaces.clear();

        let output = render_grant_review(&PluginConsentReview {
            manifest,
            prior: None,
            drift: None,
        });

        assert!(output.contains("Exports:\n  (none)\nGrants:"), "{output}");
    }

    #[test]
    fn renders_blocking_scope_expansion_as_a_drift_delta() {
        let prior = consent_record("example", '1');
        let output = render_grant_review(&PluginConsentReview {
            manifest: manifest(
                &["https://api.example.com", "https://evil.example.com"],
                '2',
            ),
            prior: Some(prior),
            drift: Some(DriftReport {
                changes: vec![DriftChange {
                    capability: "net".to_owned(),
                    permission: "egress".to_owned(),
                    kind: DriftKind::ScopeWidened,
                    before: Some(vec!["https://api.example.com".to_owned()]),
                    after: Some(vec![
                        "https://api.example.com".to_owned(),
                        "https://evil.example.com".to_owned(),
                    ]),
                }],
                export_changes: Vec::new(),
                blocks_admission: true,
            }),
        });

        assert!(output.contains("blocking permission expansion"), "{output}");
        assert!(
            output.contains("Drift since approval (BLOCKING)"),
            "{output}"
        );
        assert!(
            output.contains(
                "now ALSO requests: net.egress → https://evil.example.com (WIDENED, BLOCKING)"
            ),
            "{output}"
        );
    }

    #[test]
    fn renders_all_export_drift_deltas() {
        let output = render_grant_review(&PluginConsentReview {
            manifest: manifest(&["https://api.example.com"], '2'),
            prior: Some(consent_record("example", '1')),
            drift: Some(DriftReport {
                changes: Vec::new(),
                export_changes: vec![
                    ExportDrift {
                        name: "chap:agent/tools".to_owned(),
                        kind: ExportDriftKind::Gained,
                        before: Vec::new(),
                        after: vec!["chap:agent/tools@0.2.0".to_owned()],
                    },
                    ExportDrift {
                        name: "chap:agent/provider".to_owned(),
                        kind: ExportDriftKind::Lost,
                        before: vec!["chap:agent/provider@0.1.0".to_owned()],
                        after: Vec::new(),
                    },
                    ExportDrift {
                        name: "chap:agent/context".to_owned(),
                        kind: ExportDriftKind::VersionChanged,
                        before: vec!["chap:agent/context@0.1.0".to_owned()],
                        after: vec![
                            "chap:agent/context@0.2.0".to_owned(),
                            "chap:agent/context@0.3.0".to_owned(),
                        ],
                    },
                ],
                blocks_admission: true,
            }),
        });

        assert!(
            output.contains("now ALSO exports: chap:agent/tools@0.2.0 (NEW ROLE, BLOCKING)"),
            "{output}"
        );
        assert!(
            output.contains("no longer exports: chap:agent/provider@0.1.0 (REMOVED)"),
            "{output}"
        );
        assert!(
            output.contains(
                "exports chap:agent/context at chap:agent/context@0.2.0, chap:agent/context@0.3.0 (was chap:agent/context@0.1.0)"
            ),
            "{output}"
        );
    }
}
