mod css;
pub use css::{DisplayMode, FlexDirection, RenderNode, StyledBlock, TextAlignment, TextStyle};

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
const MAX_SCRIPTS: usize = 64;
const MAX_INLINE_SCRIPT_BYTES: usize = 64 * 1024;
const MAX_EXTERNAL_SCRIPT_BYTES: u64 = 512 * 1024;
const MAX_SCRIPT_OUTPUT_BYTES: usize = 256 * 1024;
const MAX_SCRIPT_LOOP_ITERATIONS: u64 = 10_000;
const OMITTED_ELEMENTS: &[&str] = &["noscript", "script", "style", "svg"];

#[derive(Debug, Clone, PartialEq)]
pub struct Article {
    pub title: String,
    pub text: String,
    pub blocks: Vec<StyledBlock>,
    pub render_tree: RenderNode,
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
    let initial_dom = parse_document(RcDom::default(), Default::default()).one(html.as_ref());
    let base_url = css::document_base_url(&initial_dom.document, &final_url);
    let scripts = load_scripts(
        &initial_dom.document,
        &base_url,
        &agent,
        &mut resource_warnings,
    );
    let html = execute_scripts(
        html.as_ref(),
        scripts,
        final_url.as_str(),
        &mut resource_warnings,
    );
    let dom = parse_document(RcDom::default(), Default::default()).one(html.as_str());

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
        render_tree: css::render_tree(&root, stylesheets, base_url),
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

fn load_scripts(
    document: &Handle,
    base_url: &url::Url,
    agent: &ureq::Agent,
    warnings: &mut Vec<String>,
) -> Vec<String> {
    let sources = script_sources(document);
    let mut scripts = Vec::new();
    for (index, source) in sources.iter().take(MAX_SCRIPTS).enumerate() {
        let script = match source {
            ScriptSource::Inline(source) => {
                if source.len() > MAX_INLINE_SCRIPT_BYTES {
                    warnings.push(format!(
                        "Đã bỏ qua script inline {} vì vượt giới hạn kích thước.",
                        index + 1
                    ));
                    continue;
                }
                source.clone()
            }
            ScriptSource::External(source_url) => {
                let Some(script_url) = resolve_script_url(base_url, source_url) else {
                    warnings.push(format!(
                        "Đã bỏ qua script ngoài {} vì URL không hợp lệ.",
                        index + 1
                    ));
                    continue;
                };
                match fetch_bytes(agent, &script_url, MAX_EXTERNAL_SCRIPT_BYTES) {
                    Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
                    Err(error) => {
                        warnings.push(format!("Không tải được script ngoài {script_url}: {error}"));
                        continue;
                    }
                }
            }
        };
        scripts.push(script);
    }
    if sources.len() > MAX_SCRIPTS {
        warnings.push(format!(
            "Đã bỏ qua script vượt giới hạn {MAX_SCRIPTS} script."
        ));
    }
    scripts
}

fn resolve_script_url(base_url: &url::Url, source: &str) -> Option<String> {
    let url = base_url.join(source).ok()?;
    matches!(url.scheme(), "http" | "https").then(|| url.to_string())
}

#[cfg(test)]
fn execute_inline_scripts(html: &str, warnings: &mut Vec<String>) -> String {
    let dom = parse_document(RcDom::default(), Default::default()).one(html);
    let scripts = script_sources(&dom.document)
        .into_iter()
        .filter_map(|source| match source {
            ScriptSource::Inline(source) => Some(source),
            ScriptSource::External(_) => None,
        })
        .collect();
    execute_scripts(html, scripts, "https://example.test/", warnings)
}

fn execute_scripts(
    html: &str,
    scripts: Vec<String>,
    page_url: &str,
    warnings: &mut Vec<String>,
) -> String {
    if scripts.is_empty() {
        return html.to_owned();
    }

    let dom = parse_document(RcDom::default(), Default::default()).one(html);
    let dom_model = dom_model_source(&dom.document);
    let location = location_model(page_url);
    let platform = js_string(std::env::consts::OS);
    let mut context = Context::default();
    context
        .runtime_limits_mut()
        .set_loop_iteration_limit(MAX_SCRIPT_LOOP_ITERATIONS);
    let bootstrap = format!(
        r###"
        globalThis.__dragonWrites = [];
        globalThis.__dragonRoot = {dom_model};
        globalThis.__dragonLocation = {location};
        function __dragonEscape(value, attribute) {{
            value = String(value);
            value = value.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
            return attribute ? value.replace(/"/g, "&quot;") : value;
        }}
        function __dragonSerialize(node) {{
            if (node._raw) return node.nodeValue;
            if (node.nodeType === 3) return __dragonEscape(node.nodeValue, false);
            if (node.nodeType === 8) return "<!--" + node.nodeValue + "-->";
            if (node.nodeType !== 1) return node.childNodes.map(__dragonSerialize).join("");
            var tag = node.tagName.toLowerCase();
            var output = "<" + tag;
            Object.keys(node._attributes).forEach(function(key) {{
                output += " " + key + '="' + __dragonEscape(node._attributes[key], true) + '"';
            }});
            output += ">";
            if (/^(area|base|br|col|embed|hr|img|input|link|meta|param|source|track|wbr)$/.test(tag)) return output;
            return output + node.childNodes.map(__dragonSerialize).join("") + "</" + tag + ">";
        }}
        function __dragonCreate(model, parent) {{
            var node;
            if (model[0] === "e") {{
                node = {{ nodeType: 1, tagName: model[1].toUpperCase(), nodeName: model[1].toUpperCase(),
                    parentNode: parent || null, childNodes: [], _attributes: Object.create(null) }};
                model[2].forEach(function(pair) {{ node._attributes[pair[0]] = pair[1]; }});
                model[3].forEach(function(child) {{ node.childNodes.push(__dragonCreate(child, node)); }});
            }} else {{
                node = {{ nodeType: model[0] === "c" ? 8 : (model[0] === "f" ? 9 : 3),
                    nodeName: model[0] === "c" ? "#comment" : (model[0] === "f" ? "#document" : "#text"),
                    nodeValue: model[1], parentNode: parent || null, childNodes: [], _attributes: Object.create(null) }};
                if (model[0] === "f") model[3].forEach(function(child) {{
                    node.childNodes.push(__dragonCreate(child, node));
                }});
            }}
            Object.defineProperty(node, "textContent", {{
                get: function() {{
                    if (this.nodeType === 3 || this.nodeType === 8) return this.nodeValue;
                    return this.childNodes.map(function(child) {{ return child.textContent; }}).join("");
                }},
                set: function(value) {{
                    this.childNodes.forEach(function(child) {{ child.parentNode = null; }});
                    this.childNodes = [];
                    if (this.nodeType === 3 || this.nodeType === 8) this.nodeValue = String(value);
                    else if (String(value) !== "") this.appendChild(document.createTextNode(String(value)));
                }}
            }});
            Object.defineProperty(node, "innerText", {{
                get: function() {{ return this.textContent; }},
                set: function(value) {{ this.textContent = value; }}
            }});
            Object.defineProperty(node, "innerHTML", {{
                get: function() {{ return this.childNodes.map(__dragonSerialize).join(""); }},
                set: function(value) {{
                    this.childNodes.forEach(function(child) {{ child.parentNode = null; }});
                    this.childNodes = [];
                    if (String(value) !== "") {{
                        var raw = document.createTextNode(String(value));
                        raw._raw = true;
                        this.appendChild(raw);
                    }}
                }}
            }});
            node.getAttribute = function(name) {{
                name = String(name).toLowerCase();
                return Object.prototype.hasOwnProperty.call(this._attributes, name) ? this._attributes[name] : null;
            }};
            node.hasAttribute = function(name) {{ return this.getAttribute(name) !== null; }};
            node.setAttribute = function(name, value) {{ this._attributes[String(name).toLowerCase()] = String(value); }};
            node.removeAttribute = function(name) {{ delete this._attributes[String(name).toLowerCase()]; }};
            node.addEventListener = function() {{}};
            node.removeEventListener = function() {{}};
            node.dispatchEvent = function() {{ return true; }};
            Object.defineProperty(node, "id", {{
                get: function() {{ return this.getAttribute("id") || ""; }},
                set: function(value) {{ this.setAttribute("id", value); }}
            }});
            Object.defineProperty(node, "className", {{
                get: function() {{ return this.getAttribute("class") || ""; }},
                set: function(value) {{ this.setAttribute("class", value); }}
            }});
            Object.defineProperty(node, "parentElement", {{
                get: function() {{ return this.parentNode && this.parentNode.nodeType === 1 ? this.parentNode : null; }}
            }});
            Object.defineProperty(node, "children", {{
                get: function() {{ return this.childNodes.filter(function(child) {{ return child.nodeType === 1; }}); }}
            }});
            Object.defineProperty(node, "firstChild", {{
                get: function() {{ return this.childNodes.length ? this.childNodes[0] : null; }}
            }});
            Object.defineProperty(node, "lastChild", {{
                get: function() {{ return this.childNodes.length ? this.childNodes[this.childNodes.length - 1] : null; }}
            }});
            Object.defineProperty(node, "firstElementChild", {{
                get: function() {{ return this.children[0] || null; }}
            }});
            Object.defineProperty(node, "childElementCount", {{
                get: function() {{ return this.children.length; }}
            }});
            Object.defineProperty(node, "nextSibling", {{
                get: function() {{
                    if (!this.parentNode) return null;
                    var index = this.parentNode.childNodes.indexOf(this);
                    return this.parentNode.childNodes[index + 1] || null;
                }}
            }});
            Object.defineProperty(node, "previousSibling", {{
                get: function() {{
                    if (!this.parentNode) return null;
                    var index = this.parentNode.childNodes.indexOf(this);
                    return index > 0 ? this.parentNode.childNodes[index - 1] : null;
                }}
            }});
            Object.defineProperty(node, "nextElementSibling", {{
                get: function() {{
                    var sibling = this.nextSibling;
                    while (sibling && sibling.nodeType !== 1) sibling = sibling.nextSibling;
                    return sibling;
                }}
            }});
            Object.defineProperty(node, "previousElementSibling", {{
                get: function() {{
                    var sibling = this.previousSibling;
                    while (sibling && sibling.nodeType !== 1) sibling = sibling.previousSibling;
                    return sibling;
                }}
            }});
            Object.defineProperty(node, "style", {{
                get: function() {{
                    var element = this;
                    var result = {{
                        getPropertyValue: function(name) {{
                            var declarations = (element.getAttribute("style") || "").split(";");
                            name = String(name).toLowerCase();
                            for (var i = 0; i < declarations.length; i++) {{
                                var pair = declarations[i].split(":");
                                if (pair.length > 1 && pair[0].trim().toLowerCase() === name) return pair.slice(1).join(":").trim();
                            }}
                            return "";
                        }},
                        setProperty: function(name, value) {{
                            var declarations = Object.create(null);
                            (element.getAttribute("style") || "").split(";").forEach(function(declaration) {{
                                var pair = declaration.split(":");
                                if (pair.length > 1) declarations[pair[0].trim().toLowerCase()] = pair.slice(1).join(":").trim();
                            }});
                            declarations[String(name).toLowerCase()] = String(value);
                            element.setAttribute("style", Object.keys(declarations).map(function(key) {{ return key + ": " + declarations[key]; }}).join("; "));
                        }},
                        removeProperty: function(name) {{
                            var current = this.getPropertyValue(name);
                            if (current) {{
                                var nameLower = String(name).toLowerCase();
                                var remaining = (element.getAttribute("style") || "").split(";").filter(function(declaration) {{
                                    return declaration.split(":")[0].trim().toLowerCase() !== nameLower;
                                }});
                                element.setAttribute("style", remaining.join(";"));
                            }}
                            return current;
                        }}
                    }};
                    ["display", "width", "height", "color", "backgroundColor", "fontSize", "fontWeight",
                     "textAlign", "margin", "marginTop", "marginRight", "marginBottom", "marginLeft",
                     "padding", "paddingTop", "paddingRight", "paddingBottom", "paddingLeft"].forEach(function(property) {{
                        var cssName = property.replace(/[A-Z]/g, function(letter) {{ return "-" + letter.toLowerCase(); }});
                        Object.defineProperty(result, property, {{
                            get: function() {{ return result.getPropertyValue(cssName); }},
                            set: function(value) {{ result.setProperty(cssName, value); }}
                        }});
                    }});
                    return result;
                }}
            }});
            node.contains = function(other) {{
                while (other) {{
                    if (other === this) return true;
                    other = other.parentNode;
                }}
                return false;
            }};
            node.cloneNode = function(deep) {{
                if (this.nodeType === 3) return document.createTextNode(this.nodeValue);
                if (this.nodeType === 8) return document.createComment(this.nodeValue);
                var attributes = Object.keys(this._attributes).map(function(key) {{
                    return [key, this._attributes[key]];
                }}, this);
                var copy = __dragonCreate(["e", this.tagName.toLowerCase(), attributes, []], null);
                if (deep) this.childNodes.forEach(function(child) {{ copy.appendChild(child.cloneNode(true)); }});
                return copy;
            }};
            node.matches = function(selector) {{ return __dragonMatches(this, selector); }};
            node.closest = function(selector) {{
                var current = this;
                while (current && current.nodeType === 1) {{
                    if (__dragonMatches(current, selector)) return current;
                    current = current.parentNode;
                }}
                return null;
            }};
            node.classList = {{
                contains: function(name) {{ return (" " + node.className.replace(/\s+/g, " ") + " ").indexOf(" " + name + " ") >= 0; }},
                add: function() {{
                    var names = node.className.split(/\s+/).filter(Boolean);
                    Array.prototype.forEach.call(arguments, function(name) {{ if (names.indexOf(name) < 0) names.push(name); }});
                    node.className = names.join(" ");
                }},
                remove: function() {{
                    var names = node.className.split(/\s+/).filter(Boolean);
                    Array.prototype.forEach.call(arguments, function(name) {{ names = names.filter(function(item) {{ return item !== name; }}); }});
                    node.className = names.join(" ");
                }},
                toggle: function(name, force) {{
                    var present = this.contains(name);
                    var add = force === undefined ? !present : !!force;
                    if (add) this.add(name); else this.remove(name);
                    return add;
                }}
            }};
            node.appendChild = function(child) {{
                if (child === this || (child.contains && child.contains(this))) throw new Error("Cannot insert a node into its descendant");
                if (child.parentNode) child.parentNode.removeChild(child);
                child.parentNode = this;
                this.childNodes.push(child);
                return child;
            }};
            node.insertBefore = function(child, reference) {{
                if (!reference) return this.appendChild(child);
                var index = this.childNodes.indexOf(reference);
                if (index < 0) throw new Error("Reference node is not a child");
                if (child === reference) return child;
                if (child === this || (child.contains && child.contains(this))) throw new Error("Cannot insert a node into its descendant");
                var previousParent = child.parentNode;
                var previousIndex = previousParent === this ? this.childNodes.indexOf(child) : -1;
                if (previousParent) previousParent.removeChild(child);
                if (previousParent === this && previousIndex < index) index--;
                child.parentNode = this;
                this.childNodes.splice(index, 0, child);
                return child;
            }};
            node.removeChild = function(child) {{
                var index = this.childNodes.indexOf(child);
                if (index < 0) throw new Error("Node is not a child");
                this.childNodes.splice(index, 1);
                child.parentNode = null;
                return child;
            }};
            node.remove = function() {{
                if (this.parentNode) this.parentNode.removeChild(this);
            }};
            node.querySelectorAll = function(selector) {{ return __dragonQuery(this, selector, false); }};
            node.querySelector = function(selector) {{ return __dragonQuery(this, selector, true)[0] || null; }};
            return node;
        }}
        function __dragonMatches(node, selector) {{
            function simpleMatch(element, part) {{
                if (!element || element.nodeType !== 1 || !part ||
                    part.indexOf(":") >= 0 || part.indexOf("[") >= 0 || part.indexOf("]") >= 0) return false;
                var id = part.match(/#([\w-]+)/);
                if (id) {{
                    if (element.getAttribute("id") !== id[1]) return false;
                    part = part.replace(id[0], "");
                }}
                var classes = part.match(/\.([\w-]+)/g) || [];
                for (var i = 0; i < classes.length; i++) {{
                    var name = classes[i].slice(1);
                    if ((" " + (element.getAttribute("class") || "").replace(/\s+/g, " ") + " ").indexOf(" " + name + " ") < 0) return false;
                }}
                part = part.replace(/\.([\w-]+)/g, "");
                return part === "" || part === "*" || element.tagName.toLowerCase() === part.toLowerCase();
            }}
            var groups = String(selector).split(",");
            for (var g = 0; g < groups.length; g++) {{
                var parts = groups[g].replace(/>/g, " > ").trim().split(/\s+/);
                if (!parts.length || !simpleMatch(node, parts[parts.length - 1])) continue;
                var current = node, i = parts.length - 1, matched = true;
                while (i > 0) {{
                    var direct = parts[i - 1] === ">";
                    if (direct) i--;
                    current = current.parentElement;
                    if (direct) {{
                        if (!simpleMatch(current, parts[i - 1])) matched = false;
                    }} else {{
                        while (current && !simpleMatch(current, parts[i - 1])) current = current.parentElement;
                        if (!current) matched = false;
                    }}
                    if (!matched) break;
                    i--;
                }}
                if (matched) return true;
            }}
            return false;
        }}
        function __dragonQuery(root, selector, firstOnly) {{
            var found = [], pending = root.childNodes.slice().reverse();
            while (pending.length) {{
                var node = pending.pop();
                if (__dragonMatches(node, selector)) {{
                    found.push(node);
                    if (firstOnly) return found;
                }}
                for (var i = node.childNodes.length - 1; i >= 0; i--) pending.push(node.childNodes[i]);
            }}
            return found;
        }}
        var document = {{
            _root: null,
            documentElement: null,
            head: null,
            body: null,
            URL: globalThis.__dragonLocation.href,
            documentURI: globalThis.__dragonLocation.href,
            baseURI: globalThis.__dragonLocation.href,
            readyState: "complete",
            write: function() {{ globalThis.__dragonWrites.push(Array.prototype.join.call(arguments, "")); }},
            writeln: function() {{ globalThis.__dragonWrites.push(Array.prototype.join.call(arguments, "") + "\n"); }},
            createElement: function(tag) {{ return __dragonCreate(["e", String(tag).toLowerCase(), [], []], null); }},
            createTextNode: function(text) {{ return __dragonCreate(["t", String(text)], null); }},
            createComment: function(text) {{ return __dragonCreate(["c", String(text)], null); }},
            getElementById: function(id) {{ return this.querySelector("#" + String(id)); }},
            querySelector: function(selector) {{
                if (__dragonMatches(this._root, selector)) return this._root;
                return this._root.querySelector(selector);
            }},
            querySelectorAll: function(selector) {{
                var found = this._root.querySelectorAll(selector);
                if (__dragonMatches(this._root, selector)) found.unshift(this._root);
                return found;
            }},
            getElementsByTagName: function(tag) {{ return this.querySelectorAll(String(tag)); }},
            getElementsByClassName: function(name) {{ return this.querySelectorAll("." + String(name).trim().replace(/\s+/g, ".")); }},
            addEventListener: function() {{}},
            removeEventListener: function() {{}},
            dispatchEvent: function() {{ return true; }}
        }};
        document._root = __dragonCreate(globalThis.__dragonRoot, null);
        document.documentElement = document.querySelector("html");
        document.head = document.querySelector("head");
        document.body = document.querySelector("body");
        globalThis.document = document;
        globalThis.window = globalThis;
        globalThis.self = globalThis;
        globalThis.navigator = {{
            userAgent: "DragonBrowser/0.0.2",
            appName: "DragonBrowser",
            platform: {platform},
            language: "vi-VN",
            languages: ["vi-VN", "vi"],
            onLine: true,
            cookieEnabled: false,
            javaEnabled: function() {{ return false; }}
        }};
        globalThis.location = globalThis.__dragonLocation;
        globalThis.console = {{
            log: function() {{}}, info: function() {{}}, warn: function() {{}},
            error: function() {{}}, debug: function() {{}}
        }};
        globalThis.addEventListener = function() {{}};
        globalThis.removeEventListener = function() {{}};
        globalThis.dispatchEvent = function() {{ return true; }};
        globalThis.setTimeout = function(callback) {{
            if (typeof callback === "function") callback.apply(globalThis, Array.prototype.slice.call(arguments, 2));
            return 1;
        }};
        globalThis.setInterval = function(callback) {{
            if (typeof callback === "function") callback.apply(globalThis, Array.prototype.slice.call(arguments, 2));
            return 1;
        }};
        globalThis.clearTimeout = function() {{}};
        globalThis.clearInterval = function() {{}};
        globalThis.requestAnimationFrame = function(callback) {{
            if (typeof callback === "function") callback(0);
            return 1;
        }};
        globalThis.cancelAnimationFrame = function() {{}};
        globalThis.__dragonSerializeBody = function() {{ return document.body.innerHTML; }};
        "###
    );
    if let Err(error) = context.eval(Source::from_bytes(&bootstrap)) {
        warnings.push(format!("Không khởi tạo được JavaScript: {error}"));
        return html.to_owned();
    }

    let mut writes = String::new();
    for (index, script) in scripts.iter().take(MAX_SCRIPTS).enumerate() {
        if let Err(error) = context.eval(Source::from_bytes(script)) {
            warnings.push(format!("Lỗi JavaScript {}: {error}", index + 1));
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
    if scripts.len() > MAX_SCRIPTS {
        warnings.push(format!(
            "Đã bỏ qua script vượt giới hạn {MAX_SCRIPTS} script."
        ));
    }

    let body_html = match context.eval(Source::from_bytes("globalThis.__dragonSerializeBody()")) {
        Ok(value) => match value.to_string(&mut context) {
            Ok(value) => value.to_std_string_escaped(),
            Err(error) => {
                warnings.push(format!(
                    "Không đọc được DOM sau khi chạy JavaScript: {error}"
                ));
                return insert_document_write(html, &writes);
            }
        },
        Err(error) => {
            warnings.push(format!(
                "Không thể dựng lại DOM sau khi chạy JavaScript: {error}"
            ));
            return insert_document_write(html, &writes);
        }
    };
    replace_body_content(html, &format!("{writes}{body_html}"))
}

fn dom_model_source(root: &Handle) -> String {
    match &root.data {
        NodeData::Element { name, attrs, .. } => {
            let attributes = attrs
                .borrow()
                .iter()
                .map(|attribute| {
                    format!(
                        "[{},{}]",
                        js_string(attribute.name.local.as_ref()),
                        js_string(&attribute.value)
                    )
                })
                .collect::<Vec<_>>()
                .join(",");
            let children = root
                .children
                .borrow()
                .iter()
                .map(dom_model_source)
                .collect::<Vec<_>>()
                .join(",");
            format!(
                "[\"e\",{},{},[{}]]",
                js_string(name.local.as_ref()),
                format!("[{}]", attributes),
                children
            )
        }
        NodeData::Text { contents } => {
            format!("[\"t\",{}]", js_string(&contents.borrow()))
        }
        NodeData::Comment { contents } => {
            format!("[\"c\",{}]", js_string(contents))
        }
        _ => {
            let children = root
                .children
                .borrow()
                .iter()
                .map(dom_model_source)
                .collect::<Vec<_>>()
                .join(",");
            format!("[\"f\",\"\",[],[{}]]", children)
        }
    }
}

fn location_model(page_url: &str) -> String {
    let Ok(url) = url::Url::parse(page_url) else {
        return "({href:\"about:blank\",protocol:\"about:\",host:\"\",hostname:\"\",port:\"\",pathname:\"blank\",search:\"\",hash:\"\",origin:\"null\"})".into();
    };
    let protocol = format!("{}:", url.scheme());
    let host = url.host_str().unwrap_or_default();
    let port = url.port().map_or_else(String::new, |port| port.to_string());
    let host_port = if port.is_empty() {
        host.to_owned()
    } else {
        format!("{host}:{port}")
    };
    let pathname = if url.path().is_empty() {
        "/"
    } else {
        url.path()
    };
    format!(
        "({{href:{},protocol:{},host:{},hostname:{},port:{},pathname:{},search:{},hash:{},origin:{}}})",
        js_string(url.as_str()),
        js_string(&protocol),
        js_string(&host_port),
        js_string(host),
        js_string(&port),
        js_string(pathname),
        js_string(&url.query().map_or_else(String::new, |query| format!("?{query}"))),
        js_string(&url.fragment().map_or_else(String::new, |fragment| format!("#{fragment}"))),
        js_string(&url.origin().ascii_serialization()),
    )
}

fn js_string(value: &str) -> String {
    let mut result = String::from("\"");
    for character in value.chars() {
        match character {
            '"' => result.push_str("\\\""),
            '\\' => result.push_str("\\\\"),
            '\n' => result.push_str("\\n"),
            '\r' => result.push_str("\\r"),
            '\t' => result.push_str("\\t"),
            '\u{08}' => result.push_str("\\b"),
            '\u{0c}' => result.push_str("\\f"),
            '\u{2028}' => result.push_str("\\u2028"),
            '\u{2029}' => result.push_str("\\u2029"),
            character if character.is_control() => {
                result.push_str(&format!("\\u{:04x}", character as u32));
            }
            character => result.push(character),
        }
    }
    result.push('"');
    result
}

fn replace_body_content(html: &str, content: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let Some(body_start) = lower.find("<body") else {
        return format!("{html}{content}");
    };
    let Some(body_open_end) = html[body_start..].find('>') else {
        return format!("{html}{content}");
    };
    let content_start = body_start + body_open_end + 1;
    let Some(body_end) = lower[content_start..].find("</body") else {
        return format!("{}{content}", &html[..content_start]);
    };
    let content_end = content_start + body_end;
    format!(
        "{}{}{}",
        &html[..content_start],
        content,
        &html[content_end..]
    )
}

enum ScriptSource {
    Inline(String),
    External(String),
}

fn script_sources(document: &Handle) -> Vec<ScriptSource> {
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
            if is_javascript && has_source {
                if let Some(source) = attribute(&node, "src") {
                    scripts.push(ScriptSource::External(source));
                }
            } else if is_javascript {
                let source = text_content_for_script(&node);
                if !source.trim().is_empty() {
                    scripts.push(ScriptSource::Inline(source));
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
    fn render_tree_preserves_nested_html_and_inline_styles() {
        let article = parse_html(
            "<style>.accent { color: #123 }</style>\
             <main><p>Hello <strong>world</strong> <em class='accent'>again</em></p></main>",
        );
        let main = &article.render_tree.children[0];
        let paragraph = &main.children[0];

        assert_eq!(main.tag.as_deref(), Some("main"));
        assert_eq!(paragraph.tag.as_deref(), Some("p"));
        assert_eq!(paragraph.children[1].tag.as_deref(), Some("strong"));
        assert_eq!(paragraph.children[1].style.bold, Some(true));
        assert_eq!(paragraph.children[3].tag.as_deref(), Some("em"));
        assert_eq!(paragraph.children[3].style.italic, true);
        assert_eq!(paragraph.children[3].style.color, Some([17, 34, 51]));
    }

    #[test]
    fn render_tree_keeps_anchor_href() {
        let article = parse_html("<body><p><a href='../story'>Read more</a></p></body>");
        let anchor = &article.render_tree.children[0].children[0];

        assert_eq!(anchor.tag.as_deref(), Some("a"));
        assert_eq!(anchor.href.as_deref(), Some("../story"));
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
    fn loads_external_scripts_and_inserts_document_write_output() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();

        let server = thread::spawn(move || {
            for _ in 0..2 {
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
                let (content_type, body): (&str, &[u8]) = if request_line.contains("/page.html") {
                    (
                        "text/html",
                        b"<html><body><script src='/app.js'></script>\
                              <script>document.write('<p>Inline script</p>')</script>\
                              </body></html>",
                    )
                } else {
                    (
                        "application/javascript",
                        b"document.write('<p>External script</p>')",
                    )
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

        let article = load_url(&format!("http://{address}/page.html")).unwrap();
        server.join().unwrap();

        assert!(article
            .blocks
            .iter()
            .any(|block| block.text == "External script"));
        assert!(article
            .blocks
            .iter()
            .any(|block| block.text == "Inline script"));
        assert!(article.resource_warnings.is_empty());
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
    fn supports_basic_dom_queries_and_mutations() {
        let mut warnings = Vec::new();
        let html = execute_inline_scripts(
            "<body><main id='content'><p class='old'>Before</p></main>\
             <script>\
             var paragraph = document.getElementById('content').querySelector('.old');\
             paragraph.textContent = 'After';\
             var extra = document.createElement('p');\
             extra.innerHTML = '<strong>Added</strong>';\
             document.getElementById('content').appendChild(extra);\
             </script></body>",
            &mut warnings,
        );

        assert!(warnings.is_empty());
        let page = parse_html(&html);
        assert!(page.blocks.iter().any(|block| block.text == "After"));
        assert!(page.blocks.iter().any(|block| block.text == "Added"));
    }

    #[test]
    fn provides_basic_navigator_location_console_and_timer_apis() {
        let mut warnings = Vec::new();
        let html = execute_inline_scripts(
            "<body><script>\
             console.log('test');\
             setTimeout(function() { document.write('<p>' + navigator.language + ' ' + location.pathname + '</p>'); }, 0);\
             </script></body>",
            &mut warnings,
        );

        assert!(warnings.is_empty());
        assert!(html.contains("vi-VN /"));
    }

    #[test]
    fn supports_descendant_queries_and_class_list_mutation() {
        let mut warnings = Vec::new();
        let html = execute_inline_scripts(
            "<body><main><p class='story'>Before</p></main>\
             <script>\
             var item = document.querySelector('main > p.story');\
             item.classList.add('selected');\
             item.textContent = item.className;\
             </script></body>",
            &mut warnings,
        );

        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(parse_html(&html).text.contains("story selected"));
    }

    #[test]
    fn moves_nodes_before_references_and_removes_detached_nodes() {
        let mut warnings = Vec::new();
        let html = execute_inline_scripts(
            "<body><main><p id='first'>First</p><p id='second'>Second</p><p id='remove'>Remove</p></main>\
             <script>\
             var first = document.getElementById('first');\
             var second = document.getElementById('second');\
             if (first.previousElementSibling !== null) throw new Error('unexpected previous sibling');\
             first.parentNode.insertBefore(second, first);\
             if (first.previousElementSibling !== second) throw new Error('node order is wrong');\
             document.getElementById('remove').remove();\
             </script></body>",
            &mut warnings,
        );

        assert!(warnings.is_empty(), "{warnings:?}");
        let page = parse_html(&html);
        assert!(page.text.find("Second").unwrap() < page.text.find("First").unwrap());
        assert!(!page.text.contains("Remove"));
    }

    #[test]
    fn exposes_document_head_and_element_style_mutation() {
        let mut warnings = Vec::new();
        let html = execute_inline_scripts(
            "<html><head><title>Test</title></head><body>\
             <p id='hide'>Visible</p><script>\
             if (!document.head || !document.documentElement || !document.body) throw new Error('document tree missing');\
             document.getElementById('hide').style.display = 'none';\
             </script></body></html>",
            &mut warnings,
        );

        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(html.contains("style=\"display: none\""));
        assert!(parse_html(&html).render_tree.children[0].style.hidden);
    }

    #[test]
    fn executes_at_most_64_scripts() {
        let scripts = (0..65)
            .map(|index| format!("<script>document.write('{index},')</script>"))
            .collect::<String>();
        let mut warnings = Vec::new();
        let html = execute_inline_scripts(&format!("<body>{scripts}</body>"), &mut warnings);

        let rendered = parse_html(&html).text;
        assert!(rendered.ends_with("63,"));
        assert!(!rendered.ends_with("64,"));
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("64"));
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
        assert!(warnings[0].contains("JavaScript"));
    }
}
