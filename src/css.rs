use markup5ever_rcdom::{Handle, NodeData};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct TextStyle {
    pub color: Option<[u8; 3]>,
    pub background_color: Option<[u8; 3]>,
    pub font_size: Option<f32>,
    pub bold: Option<bool>,
    pub text_align: Option<TextAlignment>,
    pub hidden: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextAlignment {
    Left,
    Center,
    Right,
    Justified,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StyledBlock {
    pub text: String,
    pub heading_level: Option<u8>,
    pub style: TextStyle,
    pub image_url: Option<String>,
    pub image_alt: String,
}

#[derive(Debug)]
struct Rule {
    selector: String,
    style: TextStyle,
    specificity: u8,
    order: usize,
}

pub enum StylesheetSource {
    Embedded(String),
    External(String),
}

pub fn styled_blocks(
    root: &Handle,
    stylesheets: &[String],
    base_url: Option<&url::Url>,
) -> Vec<StyledBlock> {
    let rules = parse_rules(stylesheets);
    let mut blocks = Vec::new();
    let mut pending = vec![(root.clone(), TextStyle::default())];

    while let Some((node, inherited)) = pending.pop() {
        let Some(tag) = element_name(&node) else {
            pending.extend(
                node.children
                    .borrow()
                    .iter()
                    .rev()
                    .cloned()
                    .map(|child| (child, inherited.clone())),
            );
            continue;
        };
        if matches!(tag, "script" | "noscript" | "svg") {
            continue;
        }

        let style = cascade_style(&node, &inherited, &rules);
        if style.hidden {
            continue;
        }
        if let Some(level) = heading_level(tag) {
            let text = text_content(&node);
            if !text.is_empty() {
                blocks.push(StyledBlock {
                    text,
                    heading_level: Some(level),
                    style,
                    image_url: None,
                    image_alt: String::new(),
                });
            }
            continue;
        }
        if tag == "img" {
            if let Some(image_url) = image_source(&node, base_url) {
                blocks.push(StyledBlock {
                    text: String::new(),
                    heading_level: None,
                    style,
                    image_url: Some(image_url),
                    image_alt: attribute(&node, "alt").unwrap_or_default(),
                });
            }
            continue;
        }
        if matches!(
            tag,
            "p" | "li" | "blockquote" | "pre" | "figcaption" | "td" | "th"
        ) {
            let text = text_content(&node);
            if !text.is_empty() {
                blocks.push(StyledBlock {
                    text,
                    heading_level: None,
                    style,
                    image_url: None,
                    image_alt: String::new(),
                });
            }
            continue;
        }
        if matches!(tag, "div" | "section" | "article" | "main" | "header")
            && !has_block_children(&node)
        {
            let text = text_content(&node);
            if !text.is_empty() {
                blocks.push(StyledBlock {
                    text,
                    heading_level: None,
                    style,
                    image_url: None,
                    image_alt: String::new(),
                });
            }
            continue;
        }
        pending.extend(
            node.children
                .borrow()
                .iter()
                .rev()
                .cloned()
                .map(|child| (child, style.clone())),
        );
    }

    blocks
}

pub fn embedded_stylesheets(root: &Handle) -> Vec<String> {
    let mut stylesheets = Vec::new();
    let mut pending = vec![root.clone()];
    while let Some(node) = pending.pop() {
        if element_name(&node) == Some("style") {
            stylesheets.push(text_content(&node));
            continue;
        }
        pending.extend(node.children.borrow().iter().rev().cloned());
    }
    stylesheets
}

pub fn document_stylesheets(root: &Handle, base_url: &url::Url) -> Vec<StylesheetSource> {
    let mut stylesheets = Vec::new();
    let mut pending = vec![root.clone()];
    while let Some(node) = pending.pop() {
        match element_name(&node) {
            Some("style") => {
                stylesheets.push(StylesheetSource::Embedded(text_content(&node)));
                continue;
            }
            Some("link") => {
                let rel = attribute(&node, "rel").unwrap_or_default();
                if rel
                    .split_ascii_whitespace()
                    .any(|value| value.eq_ignore_ascii_case("stylesheet"))
                {
                    if let Some(href) = attribute(&node, "href") {
                        if let Some(url) = resolve_url(Some(base_url), &href) {
                            stylesheets.push(StylesheetSource::External(url));
                        }
                    }
                }
            }
            _ => {}
        }
        pending.extend(node.children.borrow().iter().rev().cloned());
    }
    stylesheets
}

fn image_source(node: &Handle, base_url: Option<&url::Url>) -> Option<String> {
    for key in ["data-src", "data-original", "data-lazy-src", "src"] {
        if let Some(src) = attribute(node, key) {
            if let Some(url) = resolve_url(base_url, &src) {
                return Some(url);
            }
        }
    }
    attribute(node, "srcset")
        .and_then(|srcset| srcset.split(',').next().map(str::to_owned))
        .and_then(|candidate| candidate.split_ascii_whitespace().next().map(str::to_owned))
        .and_then(|src| resolve_url(base_url, &src))
}

pub fn document_base_url(root: &Handle, fallback: &url::Url) -> url::Url {
    let mut pending = vec![root.clone()];
    while let Some(node) = pending.pop() {
        if element_name(&node) == Some("base") {
            if let Some(href) = attribute(&node, "href") {
                if let Ok(url) = fallback.join(&href) {
                    if matches!(url.scheme(), "http" | "https") {
                        return url;
                    }
                }
            }
        }
        pending.extend(node.children.borrow().iter().rev().cloned());
    }
    fallback.clone()
}

pub fn image_urls(root: &Handle, base_url: &url::Url) -> Vec<String> {
    let mut urls = Vec::new();
    let mut pending = vec![root.clone()];
    while let Some(node) = pending.pop() {
        if element_name(&node) == Some("img") {
            if let Some(url) = image_source(&node, Some(base_url)) {
                if !urls.contains(&url) {
                    urls.push(url);
                }
            }
        }
        pending.extend(node.children.borrow().iter().rev().cloned());
    }
    urls
}

fn resolve_url(base_url: Option<&url::Url>, value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() || value.starts_with("data:") || value.starts_with("javascript:") {
        return None;
    }
    let resolved = match base_url {
        Some(base_url) => base_url.join(value).ok()?,
        None => url::Url::parse(value).ok()?,
    };
    matches!(resolved.scheme(), "http" | "https").then(|| resolved.to_string())
}

fn has_block_children(node: &Handle) -> bool {
    let mut pending = node.children.borrow().iter().cloned().collect::<Vec<_>>();
    while let Some(child) = pending.pop() {
        if element_name(&child).is_some_and(|tag| {
            matches!(
                tag,
                "p" | "li"
                    | "blockquote"
                    | "pre"
                    | "h1"
                    | "h2"
                    | "h3"
                    | "h4"
                    | "h5"
                    | "h6"
                    | "div"
                    | "section"
                    | "article"
                    | "main"
            )
        }) {
            return true;
        }
        pending.extend(child.children.borrow().iter().cloned());
    }
    false
}

fn parse_rules(stylesheets: &[String]) -> Vec<Rule> {
    let mut rules = Vec::new();
    let mut order = 0;
    for stylesheet in stylesheets {
        let stylesheet = strip_comments(stylesheet);
        for rule in stylesheet.split('}') {
            let Some((selectors, declarations)) = rule.split_once('{') else {
                continue;
            };
            let style = parse_declarations(declarations);
            for selector in selectors
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                rules.push(Rule {
                    selector: selector.to_owned(),
                    style: style.clone(),
                    specificity: selector_specificity(selector),
                    order,
                });
                order += 1;
            }
        }
    }
    rules
}

fn parse_declarations(declarations: &str) -> TextStyle {
    let mut style = TextStyle::default();
    for declaration in declarations.split(';') {
        let Some((property, value)) = declaration.split_once(':') else {
            continue;
        };
        let property = property.trim().to_ascii_lowercase();
        let value = value
            .split("!important")
            .next()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        match property.as_str() {
            "color" => style.color = parse_color(&value),
            "font-size" => style.font_size = parse_font_size(&value),
            "background-color" => style.background_color = parse_color(&value),
            "text-align" => {
                style.text_align = match value.as_str() {
                    "left" | "start" => Some(TextAlignment::Left),
                    "center" => Some(TextAlignment::Center),
                    "right" | "end" => Some(TextAlignment::Right),
                    "justify" => Some(TextAlignment::Justified),
                    _ => None,
                };
            }
            "display" => style.hidden = value == "none",
            "font-weight" => {
                style.bold = Some(
                    value == "bold"
                        || value == "bolder"
                        || value.parse::<u16>().is_ok_and(|weight| weight >= 600),
                );
            }
            _ => {}
        }
    }
    style
}

fn cascade_style(node: &Handle, inherited: &TextStyle, rules: &[Rule]) -> TextStyle {
    let mut style = inherited.clone();
    style.background_color = None;
    for property in [
        "color",
        "background-color",
        "font-size",
        "font-weight",
        "text-align",
        "display",
    ] {
        let winner = rules
            .iter()
            .filter(|rule| selector_matches(&rule.selector, node))
            .filter(|rule| match property {
                "color" => rule.style.color.is_some(),
                "background-color" => rule.style.background_color.is_some(),
                "font-size" => rule.style.font_size.is_some(),
                "font-weight" => rule.style.bold.is_some(),
                "text-align" => rule.style.text_align.is_some(),
                _ => rule.style.hidden,
            })
            .max_by_key(|rule| (rule.specificity, rule.order));
        if let Some(rule) = winner {
            match property {
                "color" => style.color = rule.style.color,
                "background-color" => style.background_color = rule.style.background_color,
                "font-size" => style.font_size = rule.style.font_size,
                "font-weight" => style.bold = rule.style.bold,
                "text-align" => style.text_align = rule.style.text_align,
                _ => style.hidden = rule.style.hidden,
            }
        }
    }

    if let Some(inline) = attribute(node, "style") {
        let inline = parse_declarations(&inline);
        if inline.color.is_some() {
            style.color = inline.color;
        }
        if inline.background_color.is_some() {
            style.background_color = inline.background_color;
        }
        if inline.font_size.is_some() {
            style.font_size = inline.font_size;
        }
        if inline.bold.is_some() {
            style.bold = inline.bold;
        }
        if inline.text_align.is_some() {
            style.text_align = inline.text_align;
        }
        style.hidden |= inline.hidden;
    }
    style
}

fn selector_matches(selector: &str, node: &Handle) -> bool {
    let selector = selector.trim();
    if selector.is_empty()
        || selector.contains([' ', '>', '+', '~', ':', '[', ']'])
        || selector.starts_with('@')
    {
        return false;
    }
    let Some(tag) = element_name(node) else {
        return false;
    };
    let mut selector = selector.to_owned();
    if let Some(id_start) = selector.find('#') {
        let id_end = selector[id_start + 1..]
            .find(['.', '#'])
            .map_or(selector.len(), |offset| id_start + 1 + offset);
        if attribute(node, "id").as_deref() != Some(&selector[id_start + 1..id_end]) {
            return false;
        }
        selector.replace_range(id_start..id_end, "");
    }
    let mut parts = selector.split('.');
    let tag_selector = parts.next().unwrap_or_default();
    if !tag_selector.is_empty() && tag_selector != "*" && tag_selector != tag {
        return false;
    }
    let classes = attribute(node, "class").unwrap_or_default();
    parts.all(|class| !class.is_empty() && classes.split_whitespace().any(|c| c == class))
}

fn selector_specificity(selector: &str) -> u8 {
    let ids = selector.matches('#').count().min(9) as u8;
    let classes = selector.matches('.').count().min(9) as u8;
    let tags = selector
        .split(['.', '#', ' ', '>', '+', '~', ':', '['])
        .filter(|part| !part.is_empty() && *part != "*")
        .count()
        .min(9) as u8;
    ids.saturating_mul(100)
        .saturating_add(classes.saturating_mul(10))
        .saturating_add(tags)
}

fn parse_color(value: &str) -> Option<[u8; 3]> {
    if let Some(rgb) = value
        .strip_prefix("rgb(")
        .or_else(|| value.strip_prefix("rgba("))
        .and_then(|value| value.strip_suffix(')'))
    {
        let mut channels = rgb
            .split(',')
            .take(3)
            .map(|channel| channel.trim().trim_end_matches('%').parse::<f32>().ok());
        let mut color = [0; 3];
        for channel in &mut color {
            let value = channels.next()??;
            if !value.is_finite() {
                return None;
            }
            *channel = if rgb.contains('%') {
                (value.clamp(0.0, 100.0) * 2.55).round() as u8
            } else {
                value.clamp(0.0, 255.0).round() as u8
            };
        }
        return Some(color);
    }

    let named = match value {
        "black" => Some([0, 0, 0]),
        "white" => Some([255, 255, 255]),
        "red" => Some([255, 0, 0]),
        "green" => Some([0, 128, 0]),
        "blue" => Some([0, 0, 255]),
        "gray" | "grey" => Some([128, 128, 128]),
        "orange" => Some([255, 165, 0]),
        "purple" => Some([128, 0, 128]),
        "navy" => Some([0, 0, 128]),
        "transparent" => None,
        _ => None,
    };
    if named.is_some() {
        return named;
    }
    let hex = value.strip_prefix('#')?;
    match hex.len() {
        3 => {
            let mut color = [0; 3];
            for (index, byte) in color.iter_mut().enumerate() {
                let digit = u8::from_str_radix(&hex[index..index + 1], 16).ok()?;
                *byte = digit * 17;
            }
            Some(color)
        }
        6 => Some([
            u8::from_str_radix(&hex[0..2], 16).ok()?,
            u8::from_str_radix(&hex[2..4], 16).ok()?,
            u8::from_str_radix(&hex[4..6], 16).ok()?,
        ]),
        _ => None,
    }
}

fn parse_font_size(value: &str) -> Option<f32> {
    let value = value.strip_suffix("px")?.trim().parse::<f32>().ok()?;
    (value.is_finite() && (6.0..=96.0).contains(&value)).then_some(value)
}

fn heading_level(tag: &str) -> Option<u8> {
    match tag {
        "h1" => Some(1),
        "h2" => Some(2),
        "h3" => Some(3),
        "h4" => Some(4),
        "h5" => Some(5),
        "h6" => Some(6),
        _ => None,
    }
}

fn strip_comments(stylesheet: &str) -> String {
    let mut result = String::new();
    let mut remaining = stylesheet;
    while let Some(start) = remaining.find("/*") {
        result.push_str(&remaining[..start]);
        let after_start = &remaining[start + 2..];
        let Some(end) = after_start.find("*/") else {
            return result;
        };
        remaining = &after_start[end + 2..];
    }
    result.push_str(remaining);
    result
}

fn text_content(root: &Handle) -> String {
    let mut text = String::new();
    let mut pending = vec![root.clone()];
    while let Some(node) = pending.pop() {
        match &node.data {
            NodeData::Text { contents } => {
                if !text.is_empty() {
                    text.push(' ');
                }
                text.push_str(contents.borrow().trim());
            }
            NodeData::Element { .. }
                if element_name(&node)
                    .is_some_and(|tag| matches!(tag, "script" | "noscript" | "svg")) =>
            {
                continue;
            }
            _ => {}
        }
        pending.extend(node.children.borrow().iter().rev().cloned());
    }
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn element_name(node: &Handle) -> Option<&str> {
    match &node.data {
        NodeData::Element { name, .. } => Some(name.local.as_ref()),
        _ => None,
    }
}

fn attribute(node: &Handle, key: &str) -> Option<String> {
    match &node.data {
        NodeData::Element { attrs, .. } => attrs
            .borrow()
            .iter()
            .find(|attribute| attribute.name.local.as_ref() == key)
            .map(|attribute| attribute.value.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use html5ever::{parse_document, tendril::TendrilSink};
    use markup5ever_rcdom::RcDom;

    use super::{styled_blocks, TextAlignment};

    #[test]
    fn applies_embedded_css_by_specificity_and_inherits_text_styles() {
        let stylesheet =
            "p { color: red; font-size: 15px } .lead { color: #123; font-weight: bold }";
        let html = format!("<style>{stylesheet}</style><main><p class='lead'>Bài báo</p></main>");
        let dom = parse_document(RcDom::default(), Default::default()).one(html);
        let blocks = styled_blocks(&dom.document, &[stylesheet.into()], None);

        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].text, "Bài báo");
        assert_eq!(blocks[0].style.color, Some([17, 34, 51]));
        assert_eq!(blocks[0].style.font_size, Some(15.0));
        assert_eq!(blocks[0].style.bold, Some(true));
    }

    #[test]
    fn inline_css_overrides_stylesheet_rules() {
        let stylesheet = "p { color: red }";
        let html = format!("<style>{stylesheet}</style><p style='color: blue'>Nội dung</p>");
        let dom = parse_document(RcDom::default(), Default::default()).one(html);
        let blocks = styled_blocks(&dom.document, &[stylesheet.into()], None);

        assert_eq!(blocks[0].style.color, Some([0, 0, 255]));
    }

    #[test]
    fn honors_hidden_elements_and_background_alignment() {
        let stylesheet =
            ".hidden { display: none } .story { background-color: #eee; text-align: center }";
        let html = format!(
            "<style>{stylesheet}</style><main><p class='hidden'>Ẩn</p>\
             <p class='story'>Hiện</p></main>"
        );
        let dom = parse_document(RcDom::default(), Default::default()).one(html);
        let blocks = styled_blocks(&dom.document, &[stylesheet.into()], None);

        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].text, "Hiện");
        assert_eq!(blocks[0].style.background_color, Some([238, 238, 238]));
        assert_eq!(blocks[0].style.text_align, Some(TextAlignment::Center));
    }

    #[test]
    fn parses_rgb_colors() {
        let stylesheet = "p { color: rgb(10, 20, 30) }";
        let html = format!("<style>{stylesheet}</style><p>Text</p>");
        let dom = parse_document(RcDom::default(), Default::default()).one(html);
        let blocks = styled_blocks(&dom.document, &[stylesheet.into()], None);

        assert_eq!(blocks[0].style.color, Some([10, 20, 30]));
    }
}
