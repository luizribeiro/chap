use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct Document {
    pub blocks: Vec<Block>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum Block {
    Paragraph(Vec<Inline>),
    Heading {
        level: u8,
        content: Vec<Inline>,
    },
    Quote(Vec<Block>),
    Code {
        language: Option<String>,
        content: String,
    },
    List {
        start: Option<u64>,
        items: Vec<ListItem>,
    },
    Rule,
    Table {
        alignments: Vec<TableAlignment>,
        head: Vec<TableCell>,
        rows: Vec<Vec<TableCell>>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ListItem {
    pub checked: Option<bool>,
    pub blocks: Vec<Block>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct TableCell {
    pub content: Vec<Inline>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum TableAlignment {
    None,
    Left,
    Center,
    Right,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum Inline {
    Text(String),
    Code(String),
    Emphasis(Vec<Inline>),
    Strong(Vec<Inline>),
    Strikethrough(Vec<Inline>),
    Link {
        destination: String,
        title: String,
        content: Vec<Inline>,
    },
    Image {
        destination: String,
        title: String,
        description: Vec<Inline>,
    },
    SoftBreak,
    HardBreak,
}

impl Document {
    pub fn parse(source: &str) -> Self {
        let options =
            Options::ENABLE_TABLES | Options::ENABLE_TASKLISTS | Options::ENABLE_STRIKETHROUGH;
        let mut frames = vec![Frame::Document(Vec::new())];

        for event in Parser::new_ext(source, options) {
            match event {
                Event::Start(tag) => {
                    if let Some(frame) = Frame::start(tag) {
                        frames.push(frame);
                    }
                }
                Event::End(end) => close_frame(&mut frames, end),
                Event::Text(text) | Event::Html(text) | Event::InlineHtml(text) => {
                    append_inline(&mut frames, Inline::Text(text.into_string()));
                }
                Event::Code(code) => {
                    append_inline(&mut frames, Inline::Code(code.into_string()));
                }
                Event::SoftBreak => append_inline(&mut frames, Inline::SoftBreak),
                Event::HardBreak => append_inline(&mut frames, Inline::HardBreak),
                Event::Rule => append_block(&mut frames, Block::Rule),
                Event::TaskListMarker(checked) => {
                    if let Some(Frame::Item { checked: state, .. }) = frames
                        .iter_mut()
                        .rev()
                        .find(|frame| matches!(frame, Frame::Item { .. }))
                    {
                        *state = Some(checked);
                    }
                }
                Event::FootnoteReference(name) => {
                    append_inline(&mut frames, Inline::Text(format!("[^{name}]")))
                }
                Event::InlineMath(math) => {
                    append_inline(&mut frames, Inline::Code(math.into_string()))
                }
                Event::DisplayMath(math) => append_block(
                    &mut frames,
                    Block::Code {
                        language: Some("math".to_owned()),
                        content: math.into_string(),
                    },
                ),
            }
        }

        while frames.len() > 1 {
            finish_top_frame(&mut frames);
        }

        match frames.pop() {
            Some(Frame::Document(blocks)) => Self { blocks },
            _ => unreachable!("the root document frame is always present"),
        }
    }
}

enum Frame {
    Document(Vec<Block>),
    Paragraph(Vec<Inline>),
    Heading {
        level: u8,
        content: Vec<Inline>,
    },
    BlockQuote(Vec<Block>),
    Code {
        language: Option<String>,
        content: String,
    },
    List {
        start: Option<u64>,
        items: Vec<ListItem>,
    },
    Item {
        checked: Option<bool>,
        blocks: Vec<Block>,
    },
    Table {
        alignments: Vec<TableAlignment>,
        head: Vec<TableCell>,
        rows: Vec<Vec<TableCell>>,
    },
    TableHead(Vec<TableCell>),
    TableRow(Vec<TableCell>),
    TableCell(Vec<Inline>),
    Emphasis(Vec<Inline>),
    Strong(Vec<Inline>),
    Strikethrough(Vec<Inline>),
    Link {
        destination: String,
        title: String,
        content: Vec<Inline>,
    },
    Image {
        destination: String,
        title: String,
        description: Vec<Inline>,
    },
}

impl Frame {
    fn start(tag: Tag<'_>) -> Option<Self> {
        match tag {
            Tag::Paragraph => Some(Self::Paragraph(Vec::new())),
            Tag::Heading { level, .. } => Some(Self::Heading {
                level: heading_level(level),
                content: Vec::new(),
            }),
            Tag::BlockQuote(_) => Some(Self::BlockQuote(Vec::new())),
            Tag::CodeBlock(kind) => Some(Self::Code {
                language: code_language(kind),
                content: String::new(),
            }),
            Tag::List(start) => Some(Self::List {
                start,
                items: Vec::new(),
            }),
            Tag::Item => Some(Self::Item {
                checked: None,
                blocks: Vec::new(),
            }),
            Tag::Table(alignments) => Some(Self::Table {
                alignments: alignments.into_iter().map(Into::into).collect(),
                head: Vec::new(),
                rows: Vec::new(),
            }),
            Tag::TableHead => Some(Self::TableHead(Vec::new())),
            Tag::TableRow => Some(Self::TableRow(Vec::new())),
            Tag::TableCell => Some(Self::TableCell(Vec::new())),
            Tag::Emphasis => Some(Self::Emphasis(Vec::new())),
            Tag::Strong => Some(Self::Strong(Vec::new())),
            Tag::Strikethrough => Some(Self::Strikethrough(Vec::new())),
            Tag::Link {
                dest_url, title, ..
            } => Some(Self::Link {
                destination: dest_url.into_string(),
                title: title.into_string(),
                content: Vec::new(),
            }),
            Tag::Image {
                dest_url, title, ..
            } => Some(Self::Image {
                destination: dest_url.into_string(),
                title: title.into_string(),
                description: Vec::new(),
            }),
            _ => None,
        }
    }

    fn matches_end(&self, end: TagEnd) -> bool {
        matches!(
            (self, end),
            (Self::Paragraph(_), TagEnd::Paragraph)
                | (Self::Heading { .. }, TagEnd::Heading(_))
                | (Self::BlockQuote(_), TagEnd::BlockQuote(_))
                | (Self::Code { .. }, TagEnd::CodeBlock)
                | (Self::List { .. }, TagEnd::List(_))
                | (Self::Item { .. }, TagEnd::Item)
                | (Self::Table { .. }, TagEnd::Table)
                | (Self::TableHead(_), TagEnd::TableHead)
                | (Self::TableRow(_), TagEnd::TableRow)
                | (Self::TableCell(_), TagEnd::TableCell)
                | (Self::Emphasis(_), TagEnd::Emphasis)
                | (Self::Strong(_), TagEnd::Strong)
                | (Self::Strikethrough(_), TagEnd::Strikethrough)
                | (Self::Link { .. }, TagEnd::Link)
                | (Self::Image { .. }, TagEnd::Image)
        )
    }
}

impl From<Alignment> for TableAlignment {
    fn from(alignment: Alignment) -> Self {
        match alignment {
            Alignment::None => Self::None,
            Alignment::Left => Self::Left,
            Alignment::Center => Self::Center,
            Alignment::Right => Self::Right,
        }
    }
}

fn heading_level(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

fn code_language(kind: CodeBlockKind<'_>) -> Option<String> {
    match kind {
        CodeBlockKind::Indented => None,
        CodeBlockKind::Fenced(info) => info
            .split_whitespace()
            .next()
            .filter(|language| !language.is_empty())
            .map(str::to_owned),
    }
}

fn close_frame(frames: &mut Vec<Frame>, end: TagEnd) {
    if frames.last().is_some_and(|frame| frame.matches_end(end)) {
        finish_top_frame(frames);
    }
}

fn finish_top_frame(frames: &mut Vec<Frame>) {
    let Some(frame) = frames.pop() else {
        return;
    };

    match frame {
        Frame::Document(_) => frames.push(frame),
        Frame::Paragraph(content) => append_block(frames, Block::Paragraph(content)),
        Frame::Heading { level, content } => {
            append_block(frames, Block::Heading { level, content });
        }
        Frame::BlockQuote(blocks) => append_block(frames, Block::Quote(blocks)),
        Frame::Code { language, content } => {
            append_block(frames, Block::Code { language, content })
        }
        Frame::List { start, items } => append_block(frames, Block::List { start, items }),
        Frame::Item { checked, blocks } => {
            if let Some(Frame::List { items, .. }) = frames.last_mut() {
                items.push(ListItem { checked, blocks });
            }
        }
        Frame::Table {
            alignments,
            head,
            rows,
        } => append_block(
            frames,
            Block::Table {
                alignments,
                head,
                rows,
            },
        ),
        Frame::TableHead(cells) => {
            if let Some(Frame::Table { head, .. }) = frames.last_mut() {
                *head = cells;
            }
        }
        Frame::TableRow(cells) => {
            if let Some(Frame::Table { rows, .. }) = frames.last_mut() {
                rows.push(cells);
            }
        }
        Frame::TableCell(content) => {
            let cell = TableCell { content };
            if let Some(Frame::TableHead(cells) | Frame::TableRow(cells)) = frames.last_mut() {
                cells.push(cell);
            }
        }
        Frame::Emphasis(content) => append_inline(frames, Inline::Emphasis(content)),
        Frame::Strong(content) => append_inline(frames, Inline::Strong(content)),
        Frame::Strikethrough(content) => append_inline(frames, Inline::Strikethrough(content)),
        Frame::Link {
            destination,
            title,
            content,
        } => append_inline(
            frames,
            Inline::Link {
                destination,
                title,
                content,
            },
        ),
        Frame::Image {
            destination,
            title,
            description,
        } => append_inline(
            frames,
            Inline::Image {
                destination,
                title,
                description,
            },
        ),
    }
}

fn append_block(frames: &mut [Frame], block: Block) {
    if let Some(container) = frames.iter_mut().rev().find(|frame| {
        matches!(
            frame,
            Frame::Document(_) | Frame::BlockQuote(_) | Frame::Item { .. }
        )
    }) {
        match container {
            Frame::Document(blocks) | Frame::BlockQuote(blocks) | Frame::Item { blocks, .. } => {
                blocks.push(block)
            }
            _ => unreachable!(),
        }
    }
}

fn append_inline(frames: &mut [Frame], inline: Inline) {
    if let Some(container) = frames.iter_mut().rev().find(|frame| {
        matches!(
            frame,
            Frame::Paragraph(_)
                | Frame::Heading { .. }
                | Frame::TableCell(_)
                | Frame::Emphasis(_)
                | Frame::Strong(_)
                | Frame::Strikethrough(_)
                | Frame::Link { .. }
                | Frame::Image { .. }
                | Frame::Code { .. }
                | Frame::Item { .. }
        )
    }) {
        match container {
            Frame::Paragraph(content)
            | Frame::TableCell(content)
            | Frame::Emphasis(content)
            | Frame::Strong(content)
            | Frame::Strikethrough(content)
            | Frame::Heading { content, .. }
            | Frame::Link { content, .. }
            | Frame::Image {
                description: content,
                ..
            } => content.push(inline),
            Frame::Code { content, .. } => append_code_text(content, inline),
            Frame::Item { blocks, .. } => match blocks.last_mut() {
                Some(Block::Paragraph(content)) => content.push(inline),
                _ => blocks.push(Block::Paragraph(vec![inline])),
            },
            _ => unreachable!(),
        }
    }
}

fn append_code_text(content: &mut String, inline: Inline) {
    match inline {
        Inline::Text(text) | Inline::Code(text) => content.push_str(&text),
        Inline::SoftBreak | Inline::HardBreak => content.push('\n'),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(value: &str) -> Inline {
        Inline::Text(value.to_owned())
    }

    #[test]
    fn parses_block_and_inline_formatting() {
        let document = Document::parse(
            "# Heading\n\nA **bold**, *italic*, ~~deleted~~ [link](https://example.com), and `code` paragraph.\n\n> quoted\n\n---",
        );

        assert_eq!(
            document.blocks,
            vec![
                Block::Heading {
                    level: 1,
                    content: vec![text("Heading")],
                },
                Block::Paragraph(vec![
                    text("A "),
                    Inline::Strong(vec![text("bold")]),
                    text(", "),
                    Inline::Emphasis(vec![text("italic")]),
                    text(", "),
                    Inline::Strikethrough(vec![text("deleted")]),
                    text(" "),
                    Inline::Link {
                        destination: "https://example.com".to_owned(),
                        title: String::new(),
                        content: vec![text("link")],
                    },
                    text(", and "),
                    Inline::Code("code".to_owned()),
                    text(" paragraph."),
                ]),
                Block::Quote(vec![Block::Paragraph(vec![text("quoted")])]),
                Block::Rule,
            ]
        );
    }

    #[test]
    fn parses_nested_and_task_lists() {
        let document =
            Document::parse("1. first\n2. second\n   - [x] nested done\n   - [ ] nested pending\n");

        assert_eq!(
            document.blocks,
            vec![Block::List {
                start: Some(1),
                items: vec![
                    ListItem {
                        checked: None,
                        blocks: vec![Block::Paragraph(vec![text("first")])],
                    },
                    ListItem {
                        checked: None,
                        blocks: vec![
                            Block::Paragraph(vec![text("second")]),
                            Block::List {
                                start: None,
                                items: vec![
                                    ListItem {
                                        checked: Some(true),
                                        blocks: vec![Block::Paragraph(vec![text("nested done")])],
                                    },
                                    ListItem {
                                        checked: Some(false),
                                        blocks: vec![Block::Paragraph(vec![text(
                                            "nested pending"
                                        )])],
                                    },
                                ],
                            },
                        ],
                    },
                ],
            }]
        );
    }

    #[test]
    fn parses_fenced_code_language_and_content() {
        let document = Document::parse("```rust linenos\nfn main() {}\n```\n");

        assert_eq!(
            document.blocks,
            vec![Block::Code {
                language: Some("rust".to_owned()),
                content: "fn main() {}\n".to_owned(),
            }]
        );
    }

    #[test]
    fn parses_gfm_tables() {
        let document = Document::parse(
            "| left | center | right |\n| :--- | :---: | ---: |\n| a | **b** | c |\n",
        );

        assert_eq!(
            document.blocks,
            vec![Block::Table {
                alignments: vec![
                    TableAlignment::Left,
                    TableAlignment::Center,
                    TableAlignment::Right,
                ],
                head: vec![
                    TableCell {
                        content: vec![text("left")],
                    },
                    TableCell {
                        content: vec![text("center")],
                    },
                    TableCell {
                        content: vec![text("right")],
                    },
                ],
                rows: vec![vec![
                    TableCell {
                        content: vec![text("a")],
                    },
                    TableCell {
                        content: vec![Inline::Strong(vec![text("b")])],
                    },
                    TableCell {
                        content: vec![text("c")],
                    },
                ]],
            }]
        );
    }
}
