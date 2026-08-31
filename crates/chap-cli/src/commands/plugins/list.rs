use super::super::NO_PLUGINS_CONFIGURED;
use crate::render::render_consent_error;
use chap_core::AgentBuilder;
use unicode_width::UnicodeWidthStr;

pub(super) fn plugin_list(builder: &AgentBuilder) -> Result<String, String> {
    let mut rows = Vec::new();
    for (id, component) in builder.plugins() {
        let roles = builder.plugin_roles(id).map_err(render_consent_error)?;
        rows.push([
            id.to_owned(),
            if roles.is_empty() {
                "-".to_owned()
            } else {
                roles.join(", ")
            },
            component.to_string_lossy().into_owned(),
        ]);
    }
    if rows.is_empty() {
        return Ok(NO_PLUGINS_CONFIGURED.to_owned());
    }
    Ok(render_table(&rows))
}

fn render_table(rows: &[[String; 3]]) -> String {
    const HEADERS: [&str; 3] = ["ID", "ROLES", "COMPONENT"];

    let mut widths = HEADERS.map(UnicodeWidthStr::width);
    for row in rows {
        for (index, cell) in row.iter().enumerate() {
            widths[index] = widths[index].max(UnicodeWidthStr::width(cell.as_str()));
        }
    }

    let mut output = String::new();
    push_row(&mut output, HEADERS, widths);
    for row in rows {
        push_row(&mut output, row.each_ref().map(String::as_str), widths);
    }
    output
}

fn push_row(output: &mut String, row: [&str; 3], widths: [usize; 3]) {
    for (index, cell) in row.into_iter().enumerate() {
        output.push_str(cell);
        if index < row.len() - 1 {
            output.push_str(&" ".repeat(widths[index] - UnicodeWidthStr::width(cell) + 2));
        }
    }
    output.push('\n');
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aligns_plugin_table_columns() {
        let rows = [
            ["short".into(), "provider".into(), "one.wasm".into()],
            ["much-longer".into(), "-".into(), "two.wasm".into()],
        ];

        assert_eq!(
            render_table(&rows),
            "ID           ROLES     COMPONENT\n\
             short        provider  one.wasm\n\
             much-longer  -         two.wasm\n"
        );
    }

    #[test]
    fn describes_an_empty_plugin_list() {
        let directory = tempfile::tempdir().unwrap();
        let config_path = directory.path().join("chap.json");
        std::fs::write(&config_path, "{}").unwrap();
        let builder = AgentBuilder::load(&config_path).unwrap();

        assert_eq!(
            plugin_list(&builder).unwrap(),
            "No plugins are configured.\n"
        );
    }
}
