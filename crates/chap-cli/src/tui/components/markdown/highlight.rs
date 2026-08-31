use super::layout::{RenderedLine, RenderedSpan, TextStyle};
use iocraft::prelude::{Color, Weight};
use std::sync::OnceLock;
use two_face::{
    re_exports::syntect::{
        easy::HighlightLines,
        highlighting::{FontStyle, Style, Theme},
        parsing::{SyntaxReference, SyntaxSet},
        util::LinesWithEndings,
    },
    theme::{EmbeddedLazyThemeSet, EmbeddedThemeName},
};

static SYNTAXES: OnceLock<SyntaxSet> = OnceLock::new();
static THEMES: OnceLock<EmbeddedLazyThemeSet> = OnceLock::new();

pub(super) fn highlight(language: Option<&str>, source: &str) -> Vec<RenderedLine> {
    let Some(language) = language.filter(|language| !is_plain_text(language)) else {
        return plain_lines(source);
    };
    let syntaxes = SYNTAXES.get_or_init(two_face::syntax::extra_newlines);
    let Some(syntax) = find_syntax(syntaxes, language) else {
        return plain_lines(source);
    };
    let theme = THEMES
        .get_or_init(two_face::theme::extra)
        .get(EmbeddedThemeName::Base16OceanDark);

    highlight_lines(source, syntaxes, syntax, theme).unwrap_or_else(|| plain_lines(source))
}

fn is_plain_text(language: &str) -> bool {
    matches!(
        language.to_ascii_lowercase().as_str(),
        "text" | "plain" | "plaintext" | "txt"
    )
}

fn find_syntax<'a>(syntaxes: &'a SyntaxSet, language: &str) -> Option<&'a SyntaxReference> {
    let token = match language.to_ascii_lowercase().as_str() {
        "c++" => "cpp",
        "c#" => "cs",
        "js" => "javascript",
        "jsx" => "JavaScript (Babel)",
        "shell" | "console" => "sh",
        "ts" => "typescript",
        "yml" => "yaml",
        _ => language,
    };
    syntaxes.find_syntax_by_token(token)
}

fn highlight_lines(
    source: &str,
    syntaxes: &SyntaxSet,
    syntax: &SyntaxReference,
    theme: &Theme,
) -> Option<Vec<RenderedLine>> {
    let mut highlighter = HighlightLines::new(syntax, theme);
    let mut lines = Vec::new();

    for line in LinesWithEndings::from(source) {
        let ranges = highlighter.highlight_line(line, syntaxes).ok()?;
        let mut rendered = RenderedLine::default();
        for (style, text) in ranges {
            let text = text.strip_suffix('\n').unwrap_or(text);
            let text = text.strip_suffix('\r').unwrap_or(text);
            if !text.is_empty() {
                rendered.push(RenderedSpan::new(text, style.into()));
            }
        }
        lines.push(rendered);
    }

    if lines.is_empty() {
        lines.push(RenderedLine::default());
    }
    Some(lines)
}

/// Deliberately leaves `style.background` unmapped; the layout applies `CODE_BACKGROUND` separately.
impl From<Style> for TextStyle {
    fn from(style: Style) -> Self {
        Self {
            color: Some(Color::Rgb {
                r: style.foreground.r,
                g: style.foreground.g,
                b: style.foreground.b,
            }),
            weight: if style.font_style.contains(FontStyle::BOLD) {
                Weight::Bold
            } else {
                Weight::Normal
            },
            italic: style.font_style.contains(FontStyle::ITALIC),
            underline: style.font_style.contains(FontStyle::UNDERLINE),
            ..Default::default()
        }
    }
}

fn plain_lines(source: &str) -> Vec<RenderedLine> {
    let mut lines = source
        .split_terminator('\n')
        .map(|line| {
            let mut rendered = RenderedLine::default();
            rendered.push(RenderedSpan::new(
                line.strip_suffix('\r').unwrap_or(line),
                TextStyle::default(),
            ));
            rendered
        })
        .collect::<Vec<_>>();
    if lines.is_empty() {
        lines.push(RenderedLine::default());
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlights_known_languages() {
        let lines = highlight(Some("rust"), "fn main() {}\n");

        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].plain_text(), "fn main() {}");
        assert!(lines[0].spans.iter().any(|span| span.style.color.is_some()));
    }

    #[test]
    fn leaves_unknown_languages_as_plain_text() {
        let lines = highlight(Some("definitely-not-a-language"), "some code\n");

        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].spans.len(), 1);
        assert_eq!(lines[0].plain_text(), "some code");
        assert_eq!(lines[0].spans[0].style, TextStyle::default());
    }

    #[test]
    fn supports_coding_agent_languages_beyond_syntect_defaults() {
        for language in ["toml", "typescript", "dockerfile", "terraform", "nix"] {
            assert!(
                find_syntax(
                    SYNTAXES.get_or_init(two_face::syntax::extra_newlines),
                    language
                )
                .is_some()
            );
        }
    }
}
