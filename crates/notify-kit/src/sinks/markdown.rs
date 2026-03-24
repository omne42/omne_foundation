use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Inline {
    Text(String),
    Link { text: String, href: String },
    Image { alt: String, src: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Line {
    pub inlines: Vec<Inline>,
}

#[derive(Debug, Clone)]
struct LinkCtx {
    href: String,
    text: String,
}

#[derive(Debug, Clone)]
struct ImageCtx {
    src: String,
    alt: String,
}

fn push_text(target: &mut Vec<Inline>, text: &str) {
    if text.is_empty() {
        return;
    }
    if let Some(Inline::Text(existing)) = target.last_mut() {
        existing.push_str(text);
        return;
    }
    target.push(Inline::Text(text.to_string()));
}

fn flush_line(lines: &mut Vec<Line>, current: &mut Vec<Inline>) {
    if current.is_empty() {
        return;
    }
    lines.push(Line {
        inlines: std::mem::take(current),
    });
}

pub(crate) fn parse_markdown_lines(input: &str) -> Vec<Line> {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_FOOTNOTES);

    let parser = Parser::new_ext(input, options);
    let mut lines = Vec::new();
    let mut current = Vec::new();
    let mut links: Vec<LinkCtx> = Vec::new();
    let mut images: Vec<ImageCtx> = Vec::new();
    let mut in_code_block = false;

    for event in parser {
        match event {
            Event::Start(tag) => match tag {
                Tag::Item => push_text(&mut current, "• "),
                Tag::CodeBlock(_) => in_code_block = true,
                Tag::Link { dest_url, .. } => {
                    links.push(LinkCtx {
                        href: dest_url.to_string(),
                        text: String::new(),
                    });
                }
                Tag::Image { dest_url, .. } => {
                    images.push(ImageCtx {
                        src: dest_url.to_string(),
                        alt: String::new(),
                    });
                }
                _ => {}
            },
            Event::End(tag_end) => match tag_end {
                TagEnd::Paragraph
                | TagEnd::Heading(_)
                | TagEnd::Item
                | TagEnd::CodeBlock
                | TagEnd::BlockQuote(_)
                | TagEnd::Table => {
                    if matches!(tag_end, TagEnd::CodeBlock) {
                        in_code_block = false;
                    }
                    flush_line(&mut lines, &mut current);
                }
                TagEnd::TableRow => flush_line(&mut lines, &mut current),
                TagEnd::Link => {
                    if let Some(link) = links.pop() {
                        let text = if link.text.trim().is_empty() {
                            link.href.clone()
                        } else {
                            link.text
                        };
                        current.push(Inline::Link {
                            text,
                            href: link.href,
                        });
                    }
                }
                TagEnd::Image => {
                    if let Some(image) = images.pop() {
                        current.push(Inline::Image {
                            alt: image.alt,
                            src: image.src,
                        });
                    }
                }
                _ => {}
            },
            Event::Text(text) => {
                if let Some(image) = images.last_mut() {
                    image.alt.push_str(text.as_ref());
                } else if let Some(link) = links.last_mut() {
                    link.text.push_str(text.as_ref());
                } else {
                    push_text(&mut current, text.as_ref());
                }
            }
            Event::Code(text) => {
                if let Some(image) = images.last_mut() {
                    image.alt.push_str(text.as_ref());
                } else if let Some(link) = links.last_mut() {
                    link.text.push_str(text.as_ref());
                } else {
                    push_text(&mut current, text.as_ref());
                }
            }
            Event::SoftBreak | Event::HardBreak => {
                if in_code_block {
                    push_text(&mut current, "\n");
                } else {
                    flush_line(&mut lines, &mut current);
                }
            }
            Event::Rule => {
                flush_line(&mut lines, &mut current);
                current.push(Inline::Text("---".to_string()));
                flush_line(&mut lines, &mut current);
            }
            Event::TaskListMarker(checked) => {
                if checked {
                    push_text(&mut current, "[x] ");
                } else {
                    push_text(&mut current, "[ ] ");
                }
            }
            Event::Html(html) | Event::InlineHtml(html) => {
                if let Some(image) = images.last_mut() {
                    image.alt.push_str(html.as_ref());
                } else if let Some(link) = links.last_mut() {
                    link.text.push_str(html.as_ref());
                } else {
                    push_text(&mut current, html.as_ref());
                }
            }
            _ => {}
        }
    }

    flush_line(&mut lines, &mut current);
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_links_and_images() {
        let lines =
            parse_markdown_lines("hello [lark](https://open.feishu.cn)\n\n![img](https://x/y.png)");
        assert_eq!(lines.len(), 2);

        assert_eq!(
            lines[0].inlines,
            vec![
                Inline::Text("hello ".to_string()),
                Inline::Link {
                    text: "lark".to_string(),
                    href: "https://open.feishu.cn".to_string()
                }
            ]
        );
        assert_eq!(
            lines[1].inlines,
            vec![Inline::Image {
                alt: "img".to_string(),
                src: "https://x/y.png".to_string()
            }]
        );
    }

    #[test]
    fn parses_task_list_items() {
        let lines = parse_markdown_lines("- [x] done\n- [ ] todo");
        assert_eq!(lines.len(), 2);
        assert_eq!(
            lines[0].inlines,
            vec![Inline::Text("• [x] done".to_string())]
        );
        assert_eq!(
            lines[1].inlines,
            vec![Inline::Text("• [ ] todo".to_string())]
        );
    }
}
