mod document;
mod highlight;
mod layout;

use iocraft::{
    prelude::*,
    taffy::{AvailableSpace, Size},
};
use layout::{MarkdownLayout, RenderedLine, RenderedSpan, TextStyle};
use std::sync::{Arc, Mutex};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

#[derive(Default, Props)]
pub(super) struct MarkdownProps {
    pub content: Arc<str>,
}

pub(super) struct Markdown {
    layout: Arc<Mutex<MarkdownLayout>>,
    initialized: bool,
}

impl Component for Markdown {
    type Props<'a> = MarkdownProps;

    fn new(props: &Self::Props<'_>) -> Self {
        Self {
            layout: Arc::new(Mutex::new(MarkdownLayout::new(Arc::clone(&props.content)))),
            initialized: false,
        }
    }

    fn update(
        &mut self,
        props: &mut Self::Props<'_>,
        _hooks: Hooks,
        updater: &mut ComponentUpdater,
    ) {
        let content_changed = self
            .layout
            .lock()
            .expect("markdown layout lock should not be poisoned")
            .update(Arc::clone(&props.content));
        if self.initialized && !content_changed {
            return;
        }

        self.initialized = true;
        let layout = Arc::clone(&self.layout);
        updater.set_measure_func(Box::new(move |known_size, available_space, _| {
            let width = measure_width(known_size.width, available_space.width);
            let (content_width, content_height) = layout
                .lock()
                .expect("markdown layout lock should not be poisoned")
                .layout(width)
                .size();
            Size {
                width: known_size.width.unwrap_or(content_width as f32),
                height: known_size.height.unwrap_or(content_height as f32),
            }
        }));
    }

    fn draw(&mut self, drawer: &mut ComponentDrawer<'_>) {
        let width = (drawer.layout().size.width as usize).max(1);
        let document = self
            .layout
            .lock()
            .expect("markdown layout lock should not be poisoned")
            .layout(width);
        let mut canvas = drawer.canvas();

        for (y, line) in document.lines.iter().enumerate() {
            let y = y as isize;
            if canvas.cell(0, y).is_none() {
                continue;
            }
            draw_line(&mut canvas, line, width, y);
        }
    }
}

fn measure_width(known: Option<f32>, available: AvailableSpace) -> usize {
    let width = known.unwrap_or(match available {
        AvailableSpace::Definite(width) => width,
        AvailableSpace::MinContent => 1.0,
        AvailableSpace::MaxContent => 120.0,
    });
    width.floor().max(1.0) as usize
}

fn draw_line(
    canvas: &mut iocraft::CanvasSubviewMut<'_>,
    line: &RenderedLine,
    width: usize,
    y: isize,
) {
    if let Some(background) = line.background {
        canvas.set_background_color(0, y, width, 1, background);
    }

    let mut x = 0;
    for span in &line.spans {
        draw_span(canvas, span, x, y);
        x += span.text.width() as isize;
    }
}

fn draw_span(canvas: &mut iocraft::CanvasSubviewMut<'_>, span: &RenderedSpan, x: isize, y: isize) {
    let width = span.text.width();
    if let Some(background) = span.style.background {
        canvas.set_background_color(x, y, width, 1, background);
    }

    let content = if span.style.strikethrough {
        strikethrough(&span.text)
    } else {
        span.text.clone()
    };
    canvas.set_text(x, y, &content, (&span.style).into());
    if span.style.hyperlink.is_some() {
        canvas.set_hyperlink(x, y, width, 1, span.style.hyperlink.clone());
    }
}

impl From<&TextStyle> for CanvasTextStyle {
    fn from(style: &TextStyle) -> Self {
        let mut canvas_style = Self::default();
        canvas_style.color = style.color;
        canvas_style.weight = style.weight;
        canvas_style.underline = style.underline;
        canvas_style.italic = style.italic;
        canvas_style
    }
}

fn strikethrough(text: &str) -> String {
    text.graphemes(true)
        .map(|grapheme| {
            if grapheme.chars().all(char::is_whitespace) {
                grapheme.to_owned()
            } else {
                format!("{grapheme}\u{0336}")
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_markdown_as_one_iocraft_component() {
        let canvas = element! {
            View(width: 30) {
                Markdown(content: Arc::<str>::from("# Heading\n\nA **bold** [link](https://example.com)."))
            }
        }
        .render(None);

        assert_eq!(canvas.get_text(0, 0, 30, 4), "Heading\n\nA bold link.\n");
        assert_eq!(
            canvas.cell(0, 0).unwrap().text_style().unwrap().weight,
            Weight::Bold
        );
        let link = canvas.cell(7, 2).unwrap();
        assert!(link.text_style().unwrap().underline);
        assert_eq!(link.hyperlink.as_deref(), Some("https://example.com"));
    }

    #[test]
    fn reflows_when_the_component_width_changes() {
        let content: Arc<str> = Arc::from("one two three four");
        let wide = element! {
            View(width: 20) { Markdown(content: Arc::clone(&content)) }
        }
        .to_string();
        let narrow = element! {
            View(width: 8) { Markdown(content: Arc::clone(&content)) }
        }
        .to_string();

        assert_eq!(wide, "one two three four\n");
        assert_eq!(narrow, "one two\nthree\nfour\n");
    }

    #[test]
    fn renders_strikethrough_without_an_iocraft_style_extension() {
        let output = element! {
            View(width: 10) {
                Markdown(content: Arc::<str>::from("~~gone~~"))
            }
        }
        .to_string();

        assert_eq!(output, "g\u{336}o\u{336}n\u{336}e\u{336}\n");
    }
}
