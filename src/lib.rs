mod css;
pub use css::{StyledBlock, TextAlignment, TextStyle};

use std::io::Read;
use std::time::Duration;

use boa_engine::{Context, Source};
use html5ever::{parse_document, tendril::TendrilSink};
use markup5ever_rcdom::{Handle, NodeData, RcDom};
use thiserror::Error;

const MAX_DOCUMENT_BYTES: u64 = 8 * 1024 * 1024;
const MAX_STYLESHEETS: usize = 8;
const MAX_STYLESHEET_BYTES: u64 = 512 * 1024;
const MAX_IMAGES: usize = 8;
const MAX_IMAGE_BYTES: u64 = 1024 * 1024;
const MAX_INLINE_SCRIPTS: usize = 16;
const MAX_INLINE_SCRIPT_BYTES: usize = 64 * 1024;
const MAX_SCRIPT_OUTPUT_BYTES: usize = 256 * 1024;
const MAX_SCRIPT_LOOP_ITERATIONS: u64 = 10_000;
const OMITTED_ELEMENTS: &[&str] = &["noscript", "script", "style", "svg"];

#[derive(Debug, Clone, PartialEq)]
pub struct Article {
    pub title: String,
    pub text: String,
    pub blocks: Vec<StyledBlock>,
    pub images: Vec<ArticleImage>,
    pub resource_warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArticleImage {
    pub url: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Error)]
pub enum DragonError {
    #[error("request failed: {0}")]
    Request(#[from] ureq::Error),
    #[error("failed to read response: {0}")]
    Read(#[from] std::io::Error),
    #[error("document exceeds the 8 MiB limit")]
    DocumentTooLarge,
    #[error("resource exceeds the {0} byte limit")]
    ResourceTooLarge(u64),
    #[error("invalid URL: {0}")]
    InvalidUrl(String),
}

/// Fetches a page, a bounded number of stylesheets/images, and builds a basic render tree.
pub fn load_url(url: &str) -> Result<Article, DragonError> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(20))
        .user_agent("DragonBrowser/0.1")
        .build();
    let requested_url =
        url::Url::parse(url).map_err(|error| DragonError::InvalidUrl(error.to_string()))?;
    if !matches!(requested_url.scheme(), "http" | "https") {
        return Err(DragonError::InvalidUrl(
            "only HTTP and HTTPS URLs are allowed".into(),
        ));
    }
    let response = agent.get(url).call()?;
    let final_url = url::Url::parse(response.get_url())
        .map_err(|error| DragonError::InvalidUrl(error.to_string()))?;
    let bytes = match read_bounded(response, MAX_DOCUMENT_BYTES) {
        Err(DragonError::ResourceTooLarge(_)) => return Err(DragonError::DocumentTooLarge),
        result => result?,
    };
    let html = String::from_utf8_lossy(&bytes);
    let mut resource_warnings = Vec::new();
    let html = execute_inline_scripts(html.as_ref(), &mut resource_warnings);
    let dom = parse_document(RcDom::default(), Default::default()).one(html.as_str());
    let base_url = css::document_base_url(&dom.document, &final_url);

    let mut stylesheets = Vec::new();
    let document_stylesheets = css::document_stylesheets(&dom.document, &base_url);
    let stylesheet_count = document_stylesheets
        .iter()
        .filter(|stylesheet| matches!(stylesheet, css::StylesheetSource::External(_)))
        .count();
    let mut external_stylesheet_count = 0;
    for stylesheet in document_stylesheets {
        match stylesheet {
            css::StylesheetSource::Embedded(stylesheet) => stylesheets.push(stylesheet),
            css::StylesheetSource::External(stylesheet_url) => {
                if external_stylesheet_count >= MAX_STYLESHEETS {
                    continue;
                }
                external_stylesheet_count += 1;
                match fetch_bytes(&agent, &stylesheet_url, MAX_STYLESHEET_BYTES) {
                    Ok(bytes) => stylesheets.push(String::from_utf8_lossy(&bytes).into_owned()),
                    Err(error) => resource_warnings.push(format!(
                        "Không tải được stylesheet {stylesheet_url}: {error}"
                    )),
                }
            }
        }
    }
    if stylesheet_count > MAX_STYLESHEETS {
        resource_warnings.push(format!(
            "Đã bỏ qua stylesheet vượt giới hạn {MAX_STYLESHEETS} file."
        ));
    }

    let page = parse_dom(&dom.document, &stylesheets, Some(&base_url));
    let mut images = Vec::new();
    let image_urls = css::image_urls(&dom.document, &base_url);
    if image_urls.len() > MAX_IMAGES {
        resource_warnings.push(format!("Đã bỏ qua ảnh vượt giới hạn {MAX_IMAGES} file."));
    }
    for image_url in image_urls.into_iter().take(MAX_IMAGES) {
        match fetch_bytes(&agent, &image_url, MAX_IMAGE_BYTES) {
            Ok(bytes) => images.push(ArticleImage {
                url: image_url,
                bytes,
            }),
            Err(error) => {
                resource_warnings.push(format!("Không tải được ảnh {image_url}: {error}"))
            }
        }
    }

    let mut article = page;
    article.images = images;
    article.resource_warnings = resource_warnings;
    Ok(article)
}

/// Parses an HTML document and builds a basic render tree without fetching resources.
pub fn parse_html(html: &str) -> Article {
    let dom = parse_document(RcDom::default(), Default::default()).one(html);
    let stylesheets = css::embedded_stylesheets(&dom.document);
    parse_dom(&dom.document, &stylesheets, None)
}

fn parse_dom(document: &Handle, stylesheets: &[String], base_url: Option<&url::Url>) -> Article {
    let title = find_text_in_tag(document, "title")
        .or_else(|| find_text_in_tag(document, "h1"))
        .unwrap_or_default();

    let root = find_element(document, "body").unwrap_or_else(|| document.clone());
    let text = collect_text(&root);
    let mut blocks = css::styled_blocks(&root, stylesheets, base_url);
    if blocks.is_empty() && !text.is_empty() {
        blocks.push(StyledBlock {
            text: text.clone(),
            heading_level: None,
            style: TextStyle::default(),
            image_url: None,
            image_alt: String::new(),
        });
    }

    Article {
        title,
        text,
        blocks,
        images: Vec::new(),
        resource_warnings: Vec::new(),
    }
}

fn fetch_bytes(agent: &ureq::Agent, url: &str, max_bytes: u64) -> Result<Vec<u8>, DragonError> {
    let parsed_url =
        url::Url::parse(url).map_err(|error| DragonError::InvalidUrl(error.to_string()))?;
    if !matches!(parsed_url.scheme(), "http" | "https") {
        return Err(DragonError::InvalidUrl(
            "only HTTP and HTTPS URLs are allowed".into(),
        ));
    }
    let response = agent.get(url).call()?;
    read_bounded(response, max_bytes)
}

fn read_bounded(response: ureq::Response, max_bytes: u64) -> Result<Vec<u8>, DragonError> {
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(max_bytes + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max_bytes {
        return Err(DragonError::ResourceTooLarge(max_bytes));
    }
    Ok(bytes)
}

fn find_element(root: &Handle, tag_name: &str) -> Option<Handle> {
    let mut pending = vec![root.clone()];
    while let Some(node) = pending.pop() {
        if element_name(&node) == Some(tag_name) {
            return Some(node);
        }
        pending.extend(node.children.borrow().iter().rev().cloned());
    }
    None
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

/// Executes JavaScript in an isolated ECMAScript context and returns its result.
///
/// This is the runtime foundation; browser APIs such as `document`, `window`,
/// and `fetch` are not implemented yet.
pub fn execute_javascript(source: &str) -> Result<String, String> {
    let mut context = Context::default();
    context
        .runtime_limits_mut()
        .set_loop_iteration_limit(MAX_SCRIPT_LOOP_ITERATIONS);
    let value = context
        .eval(Source::from_bytes(source))
        .map_err(|error| error.to_string())?;
    let result = value
        .to_string(&mut context)
        .map_err(|error| error.to_string())?;
    Ok(result.to_std_string_escaped())
}

fn execute_inline_scripts(html: &str, warnings: &mut Vec<String>) -> String {
    let dom = parse_document(RcDom::default(), Default::default()).one(html);
    let scripts = inline_scripts(&dom.document);
    if scripts.is_empty() {
        return html.to_owned();
    }

    let mut context = Context::default();
    context
        .runtime_limits_mut()
        .set_loop_iteration_limit(MAX_SCRIPT_LOOP_ITERATIONS);
    let bootstrap = "globalThis.__dragonWrites = []; \
        globalThis.document = { \
            write(...parts) { globalThis.__dragonWrites.push(parts.map(String).join('')); }, \
            writeln(...parts) { globalThis.__dragonWrites.push(parts.map(String).join('') + '\\n'); } \
        };";
    if let Err(error) = context.eval(Source::from_bytes(bootstrap)) {
        warnings.push(format!("Không khởi tạo được JavaScript: {error}"));
        return html.to_owned();
    }

    let mut writes = String::new();
    for (index, script) in scripts.iter().take(MAX_INLINE_SCRIPTS).enumerate() {
        if script.len() > MAX_INLINE_SCRIPT_BYTES {
            warnings.push(format!(
                "Đã bỏ qua script inline {} vì vượt giới hạn kích thước.",
                index + 1
            ));
            continue;
        }
        if let Err(error) = context.eval(Source::from_bytes(script)) {
            warnings.push(format!("Lỗi JavaScript inline {}: {error}", index + 1));
        }
        match context.eval(Source::from_bytes(
            "globalThis.__dragonWrites.splice(0).join('')",
        )) {
            Ok(value) => match value.to_string(&mut context) {
                Ok(value) => {
                    let output = value.to_std_string_escaped();
                    if writes.len().saturating_add(output.len()) > MAX_SCRIPT_OUTPUT_BYTES {
                        warnings
                            .push("Đã dừng chèn HTML do JavaScript vượt giới hạn đầu ra.".into());
                        break;
                    }
                    writes.push_str(&output);
                }
                Err(error) => warnings.push(format!(
                    "Không đọc được kết quả document.write của script {}: {error}",
                    index + 1
                )),
            },
            Err(error) => warnings.push(format!(
                "Không đọc được kết quả document.write của script {}: {error}",
                index + 1
            )),
        }
    }
    if scripts.len() > MAX_INLINE_SCRIPTS {
        warnings.push(format!(
            "Đã bỏ qua script inline vượt giới hạn {MAX_INLINE_SCRIPTS} script."
        ));
    }

    insert_document_write(html, &writes)
}

fn inline_scripts(document: &Handle) -> Vec<String> {
    let mut scripts = Vec::new();
    let mut pending = vec![document.clone()];
    while let Some(node) = pending.pop() {
        if element_name(&node) == Some("script") {
            let has_source = attribute(&node, "src").is_some();
            let script_type = attribute(&node, "type")
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase();
            let is_javascript = script_type.is_empty()
                || script_type == "text/javascript"
                || script_type == "application/javascript";
            if !has_source && is_javascript {
                let source = text_content_for_script(&node);
                if !source.trim().is_empty() {
                    scripts.push(source);
                }
            }
            continue;
        }
        pending.extend(node.children.borrow().iter().rev().cloned());
    }
    scripts
}

fn text_content_for_script(root: &Handle) -> String {
    let mut source = String::new();
    let mut pending = vec![root.clone()];
    while let Some(node) = pending.pop() {
        if let NodeData::Text { contents } = &node.data {
            source.push_str(&contents.borrow());
        }
        pending.extend(node.children.borrow().iter().rev().cloned());
    }
    source
}

fn insert_document_write(html: &str, output: &str) -> String {
    if output.is_empty() {
        return html.to_owned();
    }
    let lower = html.to_ascii_lowercase();
    if let Some(body_start) = lower.find("<body") {
        if let Some(relative_end) = html[body_start..].find('>') {
            let insert_at = body_start + relative_end + 1;
            let mut result = String::with_capacity(html.len() + output.len());
            result.push_str(&html[..insert_at]);
            result.push_str(output);
            result.push_str(&html[insert_at..]);
            return result;
        }
    }
    format!("{html}{output}")
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
    use std::io::{BufRead, BufReader, Cursor, Write};
    use std::net::TcpListener;
    use std::thread;

    use super::load_url;
    use super::{execute_inline_scripts, execute_javascript, parse_html};

    #[test]
    fn renders_body_content_and_omits_scripts() {
        let article = parse_html(
            "<html><head><title>Tin tức</title></head><body>\
             <nav>Menu</nav><main><h1>Bài báo</h1><p>Nội dung &amp; chi tiết.</p>\
             <script>không hiển thị</script></main></body></html>",
        );

        assert_eq!(article.title, "Tin tức");
        assert_eq!(article.text, "Menu Bài báo Nội dung & chi tiết.");
    }

    #[test]
    fn falls_back_to_page_text_and_h1_title() {
        let article = parse_html("<body><h1>Tiêu đề</h1><p>Đoạn văn</p></body>");

        assert_eq!(article.title, "Tiêu đề");
        assert_eq!(article.text, "Tiêu đề Đoạn văn");
    }

    #[test]
    fn applies_embedded_css_to_article_blocks() {
        let article = parse_html(
            "<style>.lead { color: #123; font-size: 15px; font-weight: bold }</style>\
             <main><p class='lead'>Nội dung tin</p></main>",
        );

        assert_eq!(article.blocks.len(), 1);
        assert_eq!(article.blocks[0].text, "Nội dung tin");
        assert_eq!(article.blocks[0].style.color, Some([17, 34, 51]));
        assert_eq!(article.blocks[0].style.font_size, Some(15.0));
        assert_eq!(article.blocks[0].style.bold, Some(true));
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

    #[test]
    fn refuses_non_web_urls() {
        let error = load_url("file:///etc/passwd").unwrap_err();
        assert!(matches!(error, super::DragonError::InvalidUrl(_)));
    }

    #[test]
    fn loads_external_stylesheets_and_images_in_document_order() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let mut png = Cursor::new(Vec::new());
        image::DynamicImage::new_rgba8(1, 1)
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        let png = png.into_inner();
        let expected_png = png.clone();

        let server = thread::spawn(move || {
            for _ in 0..3 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(&mut stream);
                let mut request_line = String::new();
                reader.read_line(&mut request_line).unwrap();
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if line == "\r\n" || line.is_empty() {
                        break;
                    }
                }
                drop(reader);
                let (content_type, body): (&str, &[u8]) = if request_line
                    .contains("/news/page.html")
                {
                    (
                            "text/html",
                            b"<html><head><title>News</title>\
                              <link rel='stylesheet' href='../site.css'>\
                              <style>.lead { color: #456 }</style></head><body>\
                              <main><h1>Headline</h1><p class='lead'>Story</p>\
                              <img data-src='../images/pixel.png' alt='Pixel'></main>\
                              <script>document.write('<p class=\"from-js\">Loaded by JS</p>')</script>\
                              </body></html>",
                        )
                } else if request_line.contains("/site.css") {
                    (
                        "text/css",
                        b".lead { color: #123; font-size: 15px } .from-js { color: green }",
                    )
                } else {
                    ("image/png", &png)
                };
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .unwrap();
                stream.write_all(body).unwrap();
            }
        });

        let article = load_url(&format!("http://{address}/news/page.html")).unwrap();
        server.join().unwrap();

        assert_eq!(article.title, "News");
        assert_eq!(article.images.len(), 1);
        assert_eq!(article.images[0].bytes, expected_png);
        assert!(article
            .blocks
            .iter()
            .any(|block| block.image_alt == "Pixel"));
        let story = article
            .blocks
            .iter()
            .find(|block| block.text == "Story")
            .unwrap();
        assert_eq!(story.style.color, Some([68, 85, 102]));
        assert_eq!(story.style.font_size, Some(15.0));
        let js_block = article
            .blocks
            .iter()
            .find(|block| block.text == "Loaded by JS")
            .unwrap();
        assert_eq!(js_block.style.color, Some([0, 128, 0]));
        assert!(article.resource_warnings.is_empty());
    }

    #[test]
    fn executes_inline_document_write_and_reports_script_errors() {
        let mut warnings = Vec::new();
        let html = execute_inline_scripts(
            "<html><body><script>document.write('<p>dynamic</p>'); throw new Error('test')</script></body></html>",
            &mut warnings,
        );

        assert!(html.contains("<p>dynamic</p>"));
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("test"));
        let page = parse_html(&html);
        assert!(page.blocks.iter().any(|block| block.text == "dynamic"));
    }

    #[test]
    fn limits_inline_script_loop_iterations() {
        let mut warnings = Vec::new();
        let html = execute_inline_scripts(
            "<body><script>while (true) {}</script><p>still loads</p></body>",
            &mut warnings,
        );

        assert!(html.contains("still loads"));
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("JavaScript inline"));
    }
}
