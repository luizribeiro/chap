use super::{
    document::{Block, Document, Inline, ListItem, TableAlignment, TableCell},
    highlight,
};
use iocraft::prelude::{Color, Weight};
use std::{collections::HashMap, sync::Arc};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

const CODE_BACKGROUND: Color = Color::Rgb {
    r: 36,
    g: 40,
    b: 48,
};

#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct TextStyle {
    pub color: Option<Color>,
    pub background: Option<Color>,
    pub weight: Weight,
    pub italic: bool,
    pub underline: bool,
    pub strikethrough: bool,
    pub hyperlink: Option<Arc<str>>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct RenderedSpan {
    pub text: String,
    pub style: TextStyle,
}

impl RenderedSpan {
    pub fn new(text: impl Into<String>, style: TextStyle) -> Self {
        Self {
            text: text.into(),
            style,
        }
    }

    fn width(&self) -> usize {
        self.text.width()
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct RenderedLine {
    pub spans: Vec<RenderedSpan>,
    pub background: Option<Color>,
}

impl RenderedLine {
    pub fn push(&mut self, span: RenderedSpan) {
        if span.text.is_empty() {
            return;
        }
        if let Some(last) = self.spans.last_mut()
            && last.style == span.style
        {
            last.text.push_str(&span.text);
            return;
        }
        self.spans.push(span);
    }

    pub fn width(&self) -> usize {
        self.spans.iter().map(RenderedSpan::width).sum()
    }

    #[cfg(test)]
    pub fn plain_text(&self) -> String {
        self.spans.iter().map(|span| span.text.as_str()).collect()
    }

    fn trim_end(&mut self) {
        while let Some(last) = self.spans.last_mut() {
            let trimmed = last.text.trim_end_matches(char::is_whitespace).len();
            last.text.truncate(trimmed);
            if last.text.is_empty() {
                self.spans.pop();
            } else {
                break;
            }
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct RenderedDocument {
    pub lines: Vec<RenderedLine>,
}

impl RenderedDocument {
    pub fn size(&self) -> (usize, usize) {
        (
            self.lines
                .iter()
                .map(RenderedLine::width)
                .max()
                .unwrap_or(0),
            self.lines.len().max(1),
        )
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct CodeKey {
    language: Option<String>,
    content: String,
}

pub(super) struct MarkdownLayout {
    source: Arc<str>,
    document: Document,
    code: HashMap<CodeKey, Arc<Vec<RenderedLine>>>,
    layouts: HashMap<usize, Arc<RenderedDocument>>,
    parse_count: usize,
    highlight_count: usize,
    layout_count: usize,
}

impl MarkdownLayout {
    pub fn new(source: Arc<str>) -> Self {
        Self {
            document: Document::parse(&source),
            source,
            code: HashMap::new(),
            layouts: HashMap::new(),
            parse_count: 1,
            highlight_count: 0,
            layout_count: 0,
        }
    }

    pub fn update(&mut self, source: Arc<str>) -> bool {
        if self.source == source {
            return false;
        }
        self.document = Document::parse(&source);
        self.source = source;
        self.code.clear();
        self.layouts.clear();
        self.parse_count += 1;
        true
    }

    pub fn layout(&mut self, width: usize) -> Arc<RenderedDocument> {
        let width = width.max(1);
        if let Some(layout) = self.layouts.get(&width) {
            return Arc::clone(layout);
        }

        let lines = layout_blocks(
            &self.document.blocks,
            width,
            &mut self.code,
            &mut self.highlight_count,
        );
        let document = Arc::new(RenderedDocument { lines });
        self.layouts.insert(width, Arc::clone(&document));
        self.layout_count += 1;
        document
    }

    #[cfg(test)]
    fn counts(&self) -> (usize, usize, usize) {
        (self.parse_count, self.highlight_count, self.layout_count)
    }
}

fn layout_blocks(
    blocks: &[Block],
    width: usize,
    code: &mut HashMap<CodeKey, Arc<Vec<RenderedLine>>>,
    highlight_count: &mut usize,
) -> Vec<RenderedLine> {
    let mut output = Vec::new();
    for block in blocks {
        let mut lines = layout_block(block, width, code, highlight_count);
        if !output.is_empty() && !lines.is_empty() {
            output.push(RenderedLine::default());
        }
        output.append(&mut lines);
    }
    if output.is_empty() {
        output.push(RenderedLine::default());
    }
    output
}

fn layout_block(
    block: &Block,
    width: usize,
    code: &mut HashMap<CodeKey, Arc<Vec<RenderedLine>>>,
    highlight_count: &mut usize,
) -> Vec<RenderedLine> {
    match block {
        Block::Paragraph(content) => {
            wrap_words(&inline_spans(content, TextStyle::default()), width)
        }
        Block::Heading { level, content } => {
            let style = TextStyle {
                color: (*level <= 2).then_some(Color::Cyan),
                weight: Weight::Bold,
                ..Default::default()
            };
            wrap_words(&inline_spans(content, style), width)
        }
        Block::Quote(blocks) => {
            let prefix = RenderedSpan::new(
                "│ ",
                TextStyle {
                    color: Some(Color::DarkGrey),
                    ..Default::default()
                },
            );
            prefix_lines(
                layout_blocks(
                    blocks,
                    width.saturating_sub(2).max(1),
                    code,
                    highlight_count,
                ),
                std::slice::from_ref(&prefix),
                std::slice::from_ref(&prefix),
            )
        }
        Block::Code { language, content } => {
            layout_code(language.as_deref(), content, width, code, highlight_count)
        }
        Block::List { start, items } => layout_list(*start, items, width, code, highlight_count),
        Block::Rule => vec![line_from_span(RenderedSpan::new(
            "─".repeat(width),
            TextStyle {
                color: Some(Color::DarkGrey),
                ..Default::default()
            },
        ))],
        Block::Table {
            alignments,
            head,
            rows,
        } => layout_table(alignments, head, rows, width),
    }
}

fn layout_code(
    language: Option<&str>,
    content: &str,
    width: usize,
    cache: &mut HashMap<CodeKey, Arc<Vec<RenderedLine>>>,
    highlight_count: &mut usize,
) -> Vec<RenderedLine> {
    let key = CodeKey {
        language: language.map(str::to_owned),
        content: content.to_owned(),
    };
    let highlighted = Arc::clone(cache.entry(key).or_insert_with(|| {
        *highlight_count += 1;
        Arc::new(highlight::highlight(language, content))
    }));
    let mut output = Vec::new();

    if let Some(language) = language {
        output.push(line_from_span(RenderedSpan::new(
            language,
            TextStyle {
                color: Some(Color::DarkGrey),
                ..Default::default()
            },
        )));
    }
    for line in highlighted.iter() {
        let mut wrapped = wrap_exact(&line.spans, width);
        for line in &mut wrapped {
            line.background = Some(CODE_BACKGROUND);
            for span in &mut line.spans {
                span.style.background = Some(CODE_BACKGROUND);
            }
        }
        output.append(&mut wrapped);
    }
    output
}

fn layout_list(
    start: Option<u64>,
    items: &[ListItem],
    width: usize,
    code: &mut HashMap<CodeKey, Arc<Vec<RenderedLine>>>,
    highlight_count: &mut usize,
) -> Vec<RenderedLine> {
    let mut output = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let marker = match item.checked {
            Some(true) => "[x] ".to_owned(),
            Some(false) => "[ ] ".to_owned(),
            None => match start {
                Some(start) => format!("{}. ", start + index as u64),
                None => "• ".to_owned(),
            },
        };
        let marker_width = marker.width();
        let content_width = width.saturating_sub(marker_width).max(1);
        let lines = layout_blocks(&item.blocks, content_width, code, highlight_count);
        let marker = RenderedSpan::new(
            marker,
            TextStyle {
                color: Some(Color::DarkGrey),
                ..Default::default()
            },
        );
        let continuation = RenderedSpan::new(" ".repeat(marker_width), TextStyle::default());
        output.extend(prefix_lines(lines, &[marker], &[continuation]));
    }
    output
}

fn layout_table(
    alignments: &[TableAlignment],
    head: &[TableCell],
    rows: &[Vec<TableCell>],
    width: usize,
) -> Vec<RenderedLine> {
    let columns = alignments
        .len()
        .max(head.len())
        .max(rows.iter().map(Vec::len).max().unwrap_or(0));
    if columns == 0 {
        return Vec::new();
    }
    let separator_width = columns.saturating_sub(1) * 3;
    if width < separator_width + columns {
        return layout_compact_table(head, rows, width);
    }
    let available = width.saturating_sub(separator_width).max(columns);
    let mut widths = vec![1; columns];
    for row in std::iter::once(head).chain(rows.iter().map(Vec::as_slice)) {
        for (index, cell) in row.iter().enumerate() {
            widths[index] = widths[index].max(inline_width(&cell.content));
        }
    }
    while widths.iter().sum::<usize>() > available {
        let Some((index, _)) = widths
            .iter()
            .enumerate()
            .filter(|(_, width)| **width > 1)
            .max_by_key(|(_, width)| **width)
        else {
            break;
        };
        widths[index] -= 1;
    }

    let mut output = layout_table_row(head, alignments, &widths, true);
    if !head.is_empty() {
        let mut separator = RenderedLine::default();
        for (index, column_width) in widths.iter().enumerate() {
            if index > 0 {
                separator.push(RenderedSpan::new(
                    "─┼─",
                    TextStyle {
                        color: Some(Color::DarkGrey),
                        ..Default::default()
                    },
                ));
            }
            separator.push(RenderedSpan::new(
                "─".repeat(*column_width),
                TextStyle {
                    color: Some(Color::DarkGrey),
                    ..Default::default()
                },
            ));
        }
        output.push(separator);
    }
    for row in rows {
        output.extend(layout_table_row(row, alignments, &widths, false));
    }
    output
}

fn layout_compact_table(
    head: &[TableCell],
    rows: &[Vec<TableCell>],
    width: usize,
) -> Vec<RenderedLine> {
    let mut output = Vec::new();
    for (index, row) in std::iter::once(head)
        .chain(rows.iter().map(Vec::as_slice))
        .enumerate()
    {
        let base = TextStyle {
            weight: if index == 0 && !head.is_empty() {
                Weight::Bold
            } else {
                Weight::Normal
            },
            ..Default::default()
        };
        let mut spans = Vec::new();
        for (cell_index, cell) in row.iter().enumerate() {
            if cell_index > 0 {
                push_span(
                    &mut spans,
                    RenderedSpan::new(
                        " | ",
                        TextStyle {
                            color: Some(Color::DarkGrey),
                            ..Default::default()
                        },
                    ),
                );
            }
            for span in inline_spans(&cell.content, base.clone()) {
                push_span(&mut spans, span);
            }
        }
        output.extend(wrap_words(&spans, width));
    }
    output
}

fn layout_table_row(
    cells: &[TableCell],
    alignments: &[TableAlignment],
    widths: &[usize],
    heading: bool,
) -> Vec<RenderedLine> {
    let base = TextStyle {
        weight: if heading {
            Weight::Bold
        } else {
            Weight::Normal
        },
        ..Default::default()
    };
    let laid_out = widths
        .iter()
        .enumerate()
        .map(|(index, width)| {
            let spans = cells
                .get(index)
                .map(|cell| inline_spans(&cell.content, base.clone()))
                .unwrap_or_default();
            wrap_words(&spans, *width)
        })
        .collect::<Vec<_>>();
    let height = laid_out.iter().map(Vec::len).max().unwrap_or(1);
    let mut output = Vec::with_capacity(height);

    for row_index in 0..height {
        let mut line = RenderedLine::default();
        for (column, width) in widths.iter().enumerate() {
            if column > 0 {
                line.push(RenderedSpan::new(
                    " │ ",
                    TextStyle {
                        color: Some(Color::DarkGrey),
                        ..Default::default()
                    },
                ));
            }
            let cell_line = laid_out[column].get(row_index).cloned().unwrap_or_default();
            append_aligned(
                &mut line,
                cell_line,
                *width,
                alignments
                    .get(column)
                    .copied()
                    .unwrap_or(TableAlignment::None),
            );
        }
        output.push(line);
    }
    output
}

fn append_aligned(
    output: &mut RenderedLine,
    line: RenderedLine,
    width: usize,
    alignment: TableAlignment,
) {
    let remaining = width.saturating_sub(line.width());
    let left = match alignment {
        TableAlignment::Right => remaining,
        TableAlignment::Center => remaining / 2,
        TableAlignment::None | TableAlignment::Left => 0,
    };
    let right = remaining - left;
    output.push(RenderedSpan::new(" ".repeat(left), TextStyle::default()));
    for span in line.spans {
        output.push(span);
    }
    output.push(RenderedSpan::new(" ".repeat(right), TextStyle::default()));
}

fn inline_width(content: &[Inline]) -> usize {
    inline_spans(content, TextStyle::default())
        .iter()
        .flat_map(|span| span.text.lines())
        .map(UnicodeWidthStr::width)
        .max()
        .unwrap_or(1)
}

fn inline_spans(content: &[Inline], base: TextStyle) -> Vec<RenderedSpan> {
    let mut output = Vec::new();
    append_inlines(&mut output, content, base);
    output
}

fn append_inlines(output: &mut Vec<RenderedSpan>, content: &[Inline], style: TextStyle) {
    for inline in content {
        match inline {
            Inline::Text(text) => push_span(output, RenderedSpan::new(text, style.clone())),
            Inline::Code(code) => {
                let mut code_style = style.clone();
                code_style.color = Some(Color::Yellow);
                code_style.background = Some(CODE_BACKGROUND);
                push_span(output, RenderedSpan::new(code, code_style));
            }
            Inline::Emphasis(content) => {
                let mut nested = style.clone();
                nested.italic = true;
                append_inlines(output, content, nested);
            }
            Inline::Strong(content) => {
                let mut nested = style.clone();
                nested.weight = Weight::Bold;
                append_inlines(output, content, nested);
            }
            Inline::Strikethrough(content) => {
                let mut nested = style.clone();
                nested.strikethrough = true;
                append_inlines(output, content, nested);
            }
            Inline::Link {
                destination,
                content,
                ..
            } => {
                let mut nested = style.clone();
                nested.color = Some(Color::Cyan);
                nested.underline = true;
                nested.hyperlink = Some(Arc::from(destination.as_str()));
                append_inlines(output, content, nested);
            }
            Inline::Image {
                destination,
                description,
                ..
            } => {
                let mut nested = style.clone();
                nested.color = Some(Color::Cyan);
                nested.underline = true;
                nested.hyperlink = Some(Arc::from(destination.as_str()));
                push_span(output, RenderedSpan::new("[image: ", nested.clone()));
                append_inlines(output, description, nested.clone());
                push_span(output, RenderedSpan::new("]", nested));
            }
            Inline::SoftBreak => push_span(output, RenderedSpan::new(" ", style.clone())),
            Inline::HardBreak => push_span(output, RenderedSpan::new("\n", style.clone())),
        }
    }
}

fn push_span(output: &mut Vec<RenderedSpan>, span: RenderedSpan) {
    if span.text.is_empty() {
        return;
    }
    if let Some(last) = output.last_mut()
        && last.style == span.style
    {
        last.text.push_str(&span.text);
    } else {
        output.push(span);
    }
}

fn wrap_words(spans: &[RenderedSpan], width: usize) -> Vec<RenderedLine> {
    let width = width.max(1);
    let mut lines = vec![RenderedLine::default()];

    for token in word_tokens(spans) {
        match token {
            WordToken::Word(spans) => {
                let token_width = spans.iter().map(RenderedSpan::width).sum::<usize>();
                let current_width = lines.last().unwrap().width();
                if current_width > 0 && current_width + token_width > width {
                    finish_line(&mut lines);
                }
                for span in spans {
                    append_fitting(&mut lines, &span.text, &span.style, width);
                }
            }
            WordToken::Space(style) => {
                let current_width = lines.last().unwrap().width();
                if current_width > 0 && current_width < width {
                    lines
                        .last_mut()
                        .unwrap()
                        .push(RenderedSpan::new(" ", style));
                }
            }
            WordToken::Break => finish_line(&mut lines),
        }
    }
    lines.last_mut().unwrap().trim_end();
    lines
}

enum WordToken {
    Word(Vec<RenderedSpan>),
    Space(TextStyle),
    Break,
}

fn word_tokens(spans: &[RenderedSpan]) -> Vec<WordToken> {
    let mut tokens = Vec::new();
    let mut word = Vec::new();

    for span in spans {
        for grapheme in span.text.graphemes(true) {
            if grapheme == "\n" {
                flush_word(&mut tokens, &mut word);
                tokens.push(WordToken::Break);
            } else if grapheme.chars().all(char::is_whitespace) {
                flush_word(&mut tokens, &mut word);
                if !matches!(tokens.last(), Some(WordToken::Space(_) | WordToken::Break)) {
                    tokens.push(WordToken::Space(span.style.clone()));
                }
            } else {
                push_span(&mut word, RenderedSpan::new(grapheme, span.style.clone()));
            }
        }
    }
    flush_word(&mut tokens, &mut word);
    tokens
}

fn flush_word(tokens: &mut Vec<WordToken>, word: &mut Vec<RenderedSpan>) {
    if !word.is_empty() {
        tokens.push(WordToken::Word(std::mem::take(word)));
    }
}

fn append_fitting(lines: &mut Vec<RenderedLine>, text: &str, style: &TextStyle, width: usize) {
    for grapheme in text.graphemes(true) {
        let grapheme_width = grapheme.width();
        if lines.last().unwrap().width() > 0
            && lines.last().unwrap().width() + grapheme_width > width
        {
            finish_line(lines);
        }
        lines
            .last_mut()
            .unwrap()
            .push(RenderedSpan::new(grapheme, style.clone()));
    }
}

fn wrap_exact(spans: &[RenderedSpan], width: usize) -> Vec<RenderedLine> {
    let width = width.max(1);
    let mut lines = vec![RenderedLine::default()];
    for span in spans {
        for grapheme in span.text.graphemes(true) {
            if grapheme == "\n" {
                lines.push(RenderedLine::default());
                continue;
            }
            let grapheme_width = grapheme.width();
            if lines.last().unwrap().width() > 0
                && lines.last().unwrap().width() + grapheme_width > width
            {
                lines.push(RenderedLine::default());
            }
            lines
                .last_mut()
                .unwrap()
                .push(RenderedSpan::new(grapheme, span.style.clone()));
        }
    }
    lines
}

fn finish_line(lines: &mut Vec<RenderedLine>) {
    lines.last_mut().unwrap().trim_end();
    lines.push(RenderedLine::default());
}

fn prefix_lines(
    lines: Vec<RenderedLine>,
    first: &[RenderedSpan],
    continuation: &[RenderedSpan],
) -> Vec<RenderedLine> {
    lines
        .into_iter()
        .enumerate()
        .map(|(index, line)| {
            let mut output = RenderedLine::default();
            for span in if index == 0 { first } else { continuation } {
                output.push(span.clone());
            }
            for span in line.spans {
                output.push(span);
            }
            output.background = line.background;
            output
        })
        .collect()
}

fn line_from_span(span: RenderedSpan) -> RenderedLine {
    let mut line = RenderedLine::default();
    line.push(span);
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(document: &RenderedDocument) -> Vec<String> {
        document
            .lines
            .iter()
            .map(RenderedLine::plain_text)
            .collect()
    }

    #[test]
    fn wraps_prose_at_word_boundaries_and_preserves_styles() {
        let mut layout = MarkdownLayout::new(Arc::from("A **bold phrase** follows."));
        let document = layout.layout(12);

        assert_eq!(plain(&document), ["A bold", "phrase", "follows."]);
        assert!(
            document.lines[0]
                .spans
                .iter()
                .any(|span| { span.text.contains("bold") && span.style.weight == Weight::Bold })
        );
    }

    #[test]
    fn keeps_punctuation_attached_to_the_preceding_word() {
        let mut layout = MarkdownLayout::new(Arc::from("hello, world"));
        let document = layout.layout(6);

        assert_eq!(plain(&document), ["hello,", "world"]);
    }

    #[test]
    fn lays_out_lists_quotes_and_task_markers() {
        let mut layout = MarkdownLayout::new(Arc::from(
            "> quoted words here\n\n- [x] finished\n- a longer pending item",
        ));
        let document = layout.layout(14);

        assert_eq!(
            plain(&document),
            [
                "│ quoted words",
                "│ here",
                "",
                "[x] finished",
                "• a longer",
                "  pending item",
            ]
        );
    }

    #[test]
    fn lays_out_and_aligns_tables() {
        let mut layout = MarkdownLayout::new(Arc::from(
            "| left | right |\n| :--- | ---: |\n| value | 42 |",
        ));
        let document = layout.layout(18);

        assert_eq!(
            plain(&document),
            ["left  │ right", "──────┼──────", "value │    42",]
        );
    }

    #[test]
    fn keeps_tables_within_very_narrow_layouts() {
        let mut layout =
            MarkdownLayout::new(Arc::from("| a | b | c |\n| - | - | - |\n| 1 | 2 | 3 |"));
        let document = layout.layout(5);

        assert!(document.lines.iter().all(|line| line.width() <= 5));
    }

    #[test]
    fn highlights_code_once_across_widths() {
        let source: Arc<str> = Arc::from("```rust\nfn main() {}\n```");
        let mut layout = MarkdownLayout::new(Arc::clone(&source));

        let wide = layout.layout(80);
        let same = layout.layout(80);
        layout.layout(8);
        assert!(Arc::ptr_eq(&wide, &same));
        assert_eq!(layout.counts(), (1, 1, 2));

        assert!(!layout.update(source));
        assert_eq!(layout.counts(), (1, 1, 2));

        assert!(layout.update(Arc::from("```rust\nlet n = 1;\n```")));
        layout.layout(80);
        assert_eq!(layout.counts(), (2, 2, 3));
    }

    #[test]
    fn wraps_unicode_by_grapheme_cluster() {
        let mut layout = MarkdownLayout::new(Arc::from("hello 👩‍💻 world"));
        let document = layout.layout(8);

        assert_eq!(plain(&document), ["hello 👩‍💻", "world"]);
    }
}
