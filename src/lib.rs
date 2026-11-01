use std::io::Read;
use std::time::Duration;

use boa_engine::{Context, Source};
use html5ever::{parse_document, tendril::TendrilSink};
use markup5ever_rcdom::{Handle, NodeData, RcDom};
use thiserror::Error;

const MAX_DOCUMENT_BYTES: u64 = 8 * 1024 * 1024;
const OMITTED_ELEMENTS: &[&str] = &[
    "aside", "button", "footer", "form", "iframe", "nav", "noscript", "script", "style", "svg",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Article {
    pub title: String,
    pub text: String,
}

#[derive(Debug, Error)]
pub enum DragonError {
    #[error("request failed: {0}")]
    Request(#[from] ureq::Error),
    #[error("failed to read response: {0}")]
    Read(#[from] std::io::Error),
    #[error("document exceeds the 8 MiB limit")]
    DocumentTooLarge,
}

/// Fetches a web page with bounded timeouts and response size, then extracts reader text.
pub fn load_url(url: &str) -> Result<Article, DragonError> {
    let response = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(20))
        .user_agent("DragonBrowser/0.1")
        .build()
        .get(url)
        .call()?;

    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(MAX_DOCUMENT_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_DOCUMENT_BYTES {
        return Err(DragonError::DocumentTooLarge);
    }

    let html = String::from_utf8_lossy(&bytes);
    Ok(parse_html(&html))
}

/// Parses HTML and prefers semantic article/main content over the full page.
pub fn parse_html(html: &str) -> Article {
    let dom = parse_document(RcDom::default(), Default::default()).one(html);
    let title = find_text_in_tag(&dom.document, "title")
        .or_else(|| find_text_in_tag(&dom.document, "h1"))
        .unwrap_or_default();

    let mut article_text = String::new();
    let mut pending = vec![dom.document.clone()];
    while let Some(node) = pending.pop() {
        if let Some(tag) = element_name(&node) {
            if tag == "article" || tag == "main" {
                let text = collect_text(&node);
                if text.len() > article_text.len() {
                    article_text = text;
                }
            }
        }
        pending.extend(node.children.borrow().iter().cloned());
    }

    let text = if article_text.is_empty() {
        collect_text(&dom.document)
    } else {
        article_text
    };

    Article { title, text }
}

/// Executes JavaScript in an isolated ECMAScript context and returns its result.
///
/// This is the runtime foundation; browser APIs such as `document`, `window`,
/// and `fetch` are not implemented yet.
pub fn execute_javascript(source: &str) -> Result<String, String> {
    let mut context = Context::default();
    let value = context
        .eval(Source::from_bytes(source))
        .map_err(|error| error.to_string())?;
    let result = value
        .to_string(&mut context)
        .map_err(|error| error.to_string())?;
    Ok(result.to_std_string_escaped())
}

fn find_text_in_tag(root: &Handle, tag_name: &str) -> Option<String> {
    let mut pending = vec![root.clone()];
    while let Some(node) = pending.pop() {
        if element_name(&node) == Some(tag_name) {
            let text = collect_text(&node);
            if !text.is_empty() {
                return Some(text);
            }
        }
        pending.extend(node.children.borrow().iter().cloned());
    }
    None
}

fn collect_text(root: &Handle) -> String {
    let mut text = String::new();
    let mut pending_space = false;
    let mut pending = vec![root.clone()];
    while let Some(node) = pending.pop() {
        match &node.data {
            NodeData::Text { contents } => {
                let contents = contents.borrow();
                if !contents.is_empty() && !text.is_empty() {
                    pending_space = true;
                }
                for character in contents.chars() {
                    if character.is_whitespace() {
                        pending_space = !text.is_empty();
                    } else {
                        if pending_space {
                            text.push(' ');
                            pending_space = false;
                        }
                        text.push(character);
                    }
                }
            }
            NodeData::Element { .. } if element_name(&node).is_some_and(is_omitted) => continue,
            _ => {}
        }
        pending.extend(node.children.borrow().iter().rev().cloned());
    }

    text
}

fn element_name(node: &Handle) -> Option<&str> {
    match &node.data {
        NodeData::Element { name, .. } => Some(name.local.as_ref()),
        _ => None,
    }
}

fn is_omitted(tag: &str) -> bool {
    OMITTED_ELEMENTS.contains(&tag)
}

#[cfg(test)]
mod tests {
    use super::{execute_javascript, parse_html};

    #[test]
    fn extracts_article_and_omits_navigation_and_scripts() {
        let article = parse_html(
            "<html><head><title>Tin tức</title></head><body>\
             <nav>Menu</nav><main><h1>Bài báo</h1><p>Nội dung &amp; chi tiết.</p>\
             <script>không hiển thị</script></main></body></html>",
        );

        assert_eq!(article.title, "Tin tức");
        assert_eq!(article.text, "Bài báo Nội dung & chi tiết.");
    }

    #[test]
    fn falls_back_to_page_text_and_h1_title() {
        let article = parse_html("<body><h1>Tiêu đề</h1><p>Đoạn văn</p></body>");

        assert_eq!(article.title, "Tiêu đề");
        assert_eq!(article.text, "Tiêu đề Đoạn văn");
    }

    #[test]
    fn executes_javascript_and_returns_the_result() {
        assert_eq!(
            execute_javascript("const title = 'Dragon'; title + ' Core'").unwrap(),
            "Dragon Core"
        );
    }

    #[test]
    fn reports_javascript_errors() {
        let error = execute_javascript("throw new Error('test failure')").unwrap_err();
        assert!(error.contains("test failure"));
    }
}
