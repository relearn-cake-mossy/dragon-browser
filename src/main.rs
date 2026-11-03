use std::collections::HashMap;
use std::io::Cursor;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

use dragon_core::{Article, RenderNode, TextAlignment, TextStyle};
use eframe::egui;

const MAX_IMAGE_PIXELS: u64 = 1_000_000;

struct DragonBrowserApp {
    tabs: Vec<BrowserTab>,
    active_tab_id: u64,
    next_tab_id: u64,
    next_request_id: u64,
    logo: Option<egui::TextureHandle>,
    logo_load_attempted: bool,
    sender: Sender<(u64, u64, Result<Article, String>)>,
    receiver: Receiver<(u64, u64, Result<Article, String>)>,
}

struct BrowserTab {
    id: u64,
    address: String,
    page: Option<Article>,
    page_images: HashMap<String, egui::TextureHandle>,
    error: Option<String>,
    loading: bool,
    request_id: u64,
}

impl BrowserTab {
    fn new(id: u64, address: String) -> Self {
        Self {
            id,
            address,
            page: None,
            page_images: HashMap::new(),
            error: None,
            loading: false,
            request_id: 0,
        }
    }

    fn label(&self) -> String {
        self.page
            .as_ref()
            .filter(|page| !page.title.is_empty())
            .map(|page| page.title.clone())
            .unwrap_or_else(|| {
                if self.address.is_empty() {
                    "Tab mới".to_owned()
                } else {
                    self.address.clone()
                }
            })
    }
}

impl DragonBrowserApp {
    fn new(initial_url: Option<String>) -> Self {
        let (sender, receiver) = mpsc::channel();
        let address = initial_url.unwrap_or_default();
        let mut initial_tab = BrowserTab::new(1, address.clone());
        initial_tab.loading = !address.is_empty();
        let mut app = Self {
            tabs: vec![initial_tab],
            active_tab_id: 1,
            next_tab_id: 2,
            next_request_id: 0,
            logo: None,
            logo_load_attempted: false,
            sender,
            receiver,
        };
        if !address.is_empty() {
            app.navigate_active();
        }
        app
    }

    fn active_tab(&self) -> Option<&BrowserTab> {
        self.tabs.iter().find(|tab| tab.id == self.active_tab_id)
    }

    fn active_tab_mut(&mut self) -> Option<&mut BrowserTab> {
        self.tabs
            .iter_mut()
            .find(|tab| tab.id == self.active_tab_id)
    }

    fn navigate_active(&mut self) {
        let Some(address) = self.active_tab().map(|tab| tab.address.clone()) else {
            return;
        };
        let address = normalize_address(&address);
        if address.is_empty() {
            return;
        }

        self.next_request_id += 1;
        let request_id = self.next_request_id;
        let Some(tab) = self.active_tab_mut() else {
            return;
        };
        tab.address = address.clone();
        tab.page = None;
        tab.page_images.clear();
        tab.error = None;
        tab.loading = true;
        let tab_id = tab.id;
        tab.request_id = request_id;
        let sender = self.sender.clone();

        thread::spawn(move || {
            let result = dragon_core::load_url(&address).map_err(|error| error.to_string());
            let _ = sender.send((tab_id, request_id, result));
        });
    }

    fn receive_page(&mut self, context: &egui::Context) {
        while let Ok((tab_id, request_id, result)) = self.receiver.try_recv() {
            let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == tab_id) else {
                continue;
            };
            if request_id != tab.request_id {
                continue;
            }
            tab.loading = false;
            match result {
                Ok(mut article) => {
                    tab.page_images.clear();
                    for image in &article.images {
                        match decode_page_image(&image.bytes) {
                            Ok(decoded_image) => {
                                let decoded_image = decoded_image.to_rgba8();
                                let size = [
                                    decoded_image.width() as usize,
                                    decoded_image.height() as usize,
                                ];
                                let pixels = decoded_image.into_raw();
                                let color_image =
                                    egui::ColorImage::from_rgba_unmultiplied(size, &pixels);
                                let texture = context.load_texture(
                                    image.url.clone(),
                                    color_image,
                                    egui::TextureOptions::LINEAR,
                                );
                                tab.page_images.insert(image.url.clone(), texture);
                            }
                            Err(error) => article
                                .resource_warnings
                                .push(format!("Không hiển thị được ảnh {}: {error}", image.url)),
                        }
                    }
                    tab.page = Some(article);
                }
                Err(error) => tab.error = Some(error),
            }
        }
    }

    fn new_tab(&mut self) {
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        self.tabs.push(BrowserTab::new(id, String::new()));
        self.active_tab_id = id;
    }

    fn close_tab(&mut self, id: u64) {
        if self.tabs.len() == 1 {
            let tab = &mut self.tabs[0];
            tab.address.clear();
            tab.page = None;
            tab.page_images.clear();
            tab.error = None;
            tab.loading = false;
            tab.request_id = 0;
            return;
        }

        let Some(index) = self.tabs.iter().position(|tab| tab.id == id) else {
            return;
        };
        self.tabs.remove(index);
        if self.active_tab_id == id {
            self.active_tab_id = self.tabs[index.min(self.tabs.len() - 1)].id;
        }
    }
}

impl eframe::App for DragonBrowserApp {
    fn update(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        if !self.logo_load_attempted {
            self.logo_load_attempted = true;
            if let Ok(image) = image::load_from_memory(include_bytes!("../assets/logo.png")) {
                let image = image.resize(64, 64, image::imageops::FilterType::Lanczos3);
                let image = image.to_rgba8();
                let size = [image.width() as usize, image.height() as usize];
                self.logo = Some(context.load_texture(
                    "dragon-logo",
                    egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw()),
                    egui::TextureOptions::LINEAR,
                ));
            }
        }
        self.receive_page(context);
        if self.tabs.iter().any(|tab| tab.loading) {
            context.request_repaint_after(Duration::from_millis(100));
        }

        egui::TopBottomPanel::top("browser_toolbar").show(context, |ui| {
            let mut selected_tab = None;
            let mut closed_tab = None;
            ui.horizontal(|ui| {
                egui::ScrollArea::horizontal()
                    .id_salt("browser_tabs")
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            for tab in &self.tabs {
                                let title = truncate_tab_title(&tab.label());
                                if ui
                                    .selectable_label(tab.id == self.active_tab_id, title)
                                    .clicked()
                                {
                                    selected_tab = Some(tab.id);
                                }
                                if ui.small_button("×").clicked() {
                                    closed_tab = Some(tab.id);
                                }
                            }
                        });
                    });
                if ui.button("+").on_hover_text("Tab mới").clicked() {
                    selected_tab = Some(0);
                }
            });
            if let Some(tab_id) = closed_tab {
                self.close_tab(tab_id);
            }
            if let Some(tab_id) = selected_tab {
                if tab_id == 0 {
                    self.new_tab();
                } else if self.tabs.iter().any(|tab| tab.id == tab_id) {
                    self.active_tab_id = tab_id;
                }
            }

            ui.horizontal(|ui| {
                if let Some(logo) = &self.logo {
                    ui.add(egui::Image::new(logo).fit_to_exact_size(egui::vec2(30.0, 30.0)));
                }
                ui.heading(
                    egui::RichText::new("Dragon").color(egui::Color32::from_rgb(194, 66, 54)),
                );
                ui.add_space(8.0);
                let address_response = if let Some(tab) = self.active_tab_mut() {
                    ui.add_sized(
                        [ui.available_width() - 88.0, 34.0],
                        egui::TextEdit::singleline(&mut tab.address)
                            .hint_text("Nhập địa chỉ trang web..."),
                    )
                } else {
                    return;
                };
                let enter_pressed = address_response.lost_focus()
                    && ui.input(|input| input.key_pressed(egui::Key::Enter));
                if ui.button("Đi").clicked() || enter_pressed {
                    self.navigate_active();
                }
            });
        });

        egui::TopBottomPanel::bottom("status_bar").show(context, |ui| {
            ui.horizontal(|ui| {
                ui.label("Dragon renderer tự viết");
                ui.separator();
                ui.label(if self.active_tab().is_some_and(|tab| tab.loading) {
                    "Đang tải trang..."
                } else {
                    "HTML/CSS/JavaScript hỗ trợ giới hạn"
                });
            });
        });

        let mut clicked_link = None;
        let current_address = self
            .active_tab()
            .map(|tab| tab.address.clone())
            .unwrap_or_default();
        egui::CentralPanel::default().show(context, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.add_space(20.0);
                    if let Some(tab) = self.active_tab() {
                        if let Some(error) = &tab.error {
                            ui.heading("Không thể mở trang");
                            ui.label(error);
                            return;
                        }
                        if let Some(article) = &tab.page {
                            for warning in &article.resource_warnings {
                                ui.colored_label(egui::Color32::DARK_RED, warning);
                            }
                            render_node(
                                ui,
                                &article.render_tree,
                                &tab.page_images,
                                &mut clicked_link,
                            );
                        } else if tab.loading {
                            ui.spinner();
                            ui.label("Dragon đang tải nội dung...");
                        } else {
                            ui.vertical_centered(|ui| {
                                ui.add_space(100.0);
                                ui.heading(
                                    egui::RichText::new("Chào mừng đến với Dragon Browser")
                                        .size(26.0),
                                );
                                ui.add_space(8.0);
                                ui.label("Nhập địa chỉ trang web ở thanh phía trên để bắt đầu.");
                            });
                        }
                    }
                });
        });
        if let Some(href) = clicked_link {
            if let Some(address) = resolve_link(&current_address, &href) {
                if let Some(tab) = self.active_tab_mut() {
                    tab.address = address;
                }
                self.navigate_active();
            }
        }
    }
}

fn render_node(
    ui: &mut egui::Ui,
    node: &RenderNode,
    images: &HashMap<String, egui::TextureHandle>,
    clicked_link: &mut Option<String>,
) {
    if node.tag.is_none() {
        render_inline(ui, node, images, clicked_link);
        return;
    }

    let tag = node.tag.as_deref().unwrap_or_default();
    if tag == "img" {
        render_image(ui, node, images, clicked_link);
        return;
    }
    let display = node.style.display;
    if display == Some(dragon_core::DisplayMode::Inline)
        || (display.is_none() && !is_block_element(tag))
    {
        render_inline(ui, node, images, clicked_link);
        return;
    }

    let spacing = if node.style.margin != [0.0; 4] {
        node.style.margin
    } else {
        let vertical = match tag {
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => 12.0,
            "p" | "blockquote" | "pre" | "ul" | "ol" | "table" | "figure" => 10.0,
            _ => 0.0,
        };
        [vertical, 0.0, vertical, 0.0]
    };
    ui.add_space(spacing[0].max(0.0));
    let mut draw_contents = |ui: &mut egui::Ui| {
        if display == Some(dragon_core::DisplayMode::Flex) {
            ui.horizontal_wrapped(|ui| render_flex_children(ui, node, images, clicked_link));
        } else if tag == "li" {
            ui.horizontal_wrapped(|ui| {
                ui.label("• ");
                render_flow_children(ui, node, images, clicked_link);
            });
        } else {
            render_flow_children(ui, node, images, clicked_link);
        }
    };
    let mut draw_aligned = |ui: &mut egui::Ui| {
        let mut contents = |ui: &mut egui::Ui| {
            ui.set_max_width(content_width(
                ui.available_width(),
                node.style.width,
                node.style.max_width,
            ));
            draw_contents(ui);
        };
        match node.style.text_align {
            Some(TextAlignment::Center) => {
                ui.with_layout(egui::Layout::top_down(egui::Align::Center), contents);
            }
            Some(TextAlignment::Right) => {
                ui.with_layout(egui::Layout::top_down(egui::Align::RIGHT), contents);
            }
            _ => contents(ui),
        }
    };
    let padding = node.style.padding;
    let inner_margin = egui::Margin {
        left: padding[3].round().clamp(0.0, 127.0) as i8,
        right: padding[1].round().clamp(0.0, 127.0) as i8,
        top: padding[0].round().clamp(0.0, 127.0) as i8,
        bottom: padding[2].round().clamp(0.0, 127.0) as i8,
    };
    let draw_box = |ui: &mut egui::Ui| {
        if let Some([red, green, blue]) = node.style.background_color {
            egui::Frame::new()
                .fill(egui::Color32::from_rgb(red, green, blue))
                .inner_margin(inner_margin)
                .show(ui, draw_aligned);
        } else if padding != [0.0; 4] {
            egui::Frame::new()
                .inner_margin(inner_margin)
                .show(ui, draw_aligned);
        } else {
            draw_aligned(ui);
        }
    };
    let available = ui.available_width().max(1.0);
    let width = content_width(available, node.style.width, node.style.max_width);
    let auto_center = node.style.width.is_some() || node.style.max_width.is_some();
    let left = spacing[3].max(0.0)
        + if auto_center {
            (available - width).max(0.0) / 2.0
        } else {
            0.0
        };
    if left > 0.0 || spacing[1] > 0.0 || auto_center {
        ui.horizontal(|ui| {
            ui.add_space(left);
            ui.allocate_ui_with_layout(
                egui::vec2(width.min(ui.available_width()).max(1.0), 0.0),
                egui::Layout::top_down(egui::Align::Min),
                draw_box,
            );
            ui.add_space(spacing[1].max(0.0));
        });
    } else {
        draw_box(ui);
    }
    ui.add_space(spacing[2].max(0.0));
}

fn content_width(available: f32, width: Option<f32>, max_width: Option<f32>) -> f32 {
    let resolve = |value: f32| {
        if value < 0.0 {
            available * (-value / 100.0).clamp(0.0, 1.0)
        } else {
            value
        }
    };
    let width = width.map(resolve).unwrap_or(available);
    let max_width = max_width.map(resolve).unwrap_or(available);
    width.min(max_width).min(available).max(1.0)
}

fn render_flex_children(
    ui: &mut egui::Ui,
    node: &RenderNode,
    images: &HashMap<String, egui::TextureHandle>,
    clicked_link: &mut Option<String>,
) {
    for child in &node.children {
        render_node(ui, child, images, clicked_link);
    }
}

fn render_flow_children(
    ui: &mut egui::Ui,
    node: &RenderNode,
    images: &HashMap<String, egui::TextureHandle>,
    clicked_link: &mut Option<String>,
) {
    let mut index = 0;
    while index < node.children.len() {
        if is_block_node(&node.children[index]) {
            render_node(ui, &node.children[index], images, clicked_link);
            index += 1;
            continue;
        }

        let start = index;
        while index < node.children.len() && !is_block_node(&node.children[index]) {
            index += 1;
        }
        ui.horizontal_wrapped(|ui| {
            for child in &node.children[start..index] {
                render_inline(ui, child, images, clicked_link);
            }
        });
    }
}

fn render_inline(
    ui: &mut egui::Ui,
    node: &RenderNode,
    images: &HashMap<String, egui::TextureHandle>,
    clicked_link: &mut Option<String>,
) {
    if node.image_url.is_some() {
        render_image(ui, node, images, clicked_link);
        return;
    }
    if let Some(href) = &node.href {
        let text = inline_text(node);
        if !text.is_empty() {
            let response = ui
                .add(
                    egui::Label::new(rich_text(&text, &node.style).underline())
                        .sense(egui::Sense::click())
                        .wrap(),
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            if response.clicked() {
                *clicked_link = Some(href.clone());
            }
        }
        return;
    }
    if node.tag.is_none() {
        let text = collapse_whitespace(&node.text);
        if !text.is_empty() {
            ui.add(egui::Label::new(rich_text(&text, &node.style)).wrap());
        }
        return;
    }

    let tag = node.tag.as_deref().unwrap_or_default();
    if matches!(tag, "br" | "hr") {
        ui.end_row();
        return;
    }
    if node.children.is_empty() {
        if matches!(tag, "input" | "button" | "select" | "textarea") {
            let value = node
                .children
                .iter()
                .map(|child| child.text.as_str())
                .collect::<String>();
            ui.label(rich_text(&value, &node.style));
        }
        return;
    }

    if let Some([red, green, blue]) = node.style.background_color {
        egui::Frame::new()
            .fill(egui::Color32::from_rgb(red, green, blue))
            .show(ui, |ui| {
                render_inline_children(ui, node, images, clicked_link);
            });
    } else {
        render_inline_children(ui, node, images, clicked_link);
    }
}

fn render_inline_children(
    ui: &mut egui::Ui,
    node: &RenderNode,
    images: &HashMap<String, egui::TextureHandle>,
    clicked_link: &mut Option<String>,
) {
    for child in &node.children {
        render_inline(ui, child, images, clicked_link);
    }
}

fn render_image(
    ui: &mut egui::Ui,
    node: &RenderNode,
    images: &HashMap<String, egui::TextureHandle>,
    clicked_link: &mut Option<String>,
) {
    if let Some(image_url) = &node.image_url {
        if let Some(texture) = images.get(image_url) {
            let image = egui::Image::new(texture).max_width(ui.available_width());
            let response = if node.href.is_some() {
                ui.add(image.sense(egui::Sense::click()))
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
            } else {
                ui.add(image)
            };
            if response.clicked() {
                *clicked_link = node.href.clone();
            }
        } else if !node.image_alt.is_empty() {
            ui.label(&node.image_alt);
        }
    }
}

fn inline_text(node: &RenderNode) -> String {
    let mut text = node.text.clone();
    for child in &node.children {
        text.push_str(&inline_text(child));
    }
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn resolve_link(current_address: &str, href: &str) -> Option<String> {
    let current = url::Url::parse(current_address).ok()?;
    let target = current.join(href.trim()).ok()?;
    matches!(target.scheme(), "http" | "https").then(|| target.to_string())
}

fn rich_text(text: &str, style: &TextStyle) -> egui::RichText {
    let mut text = egui::RichText::new(text).size(style.font_size.unwrap_or(16.0));
    if style.bold == Some(true) {
        text = text.strong();
    }
    if style.italic {
        text = text.italics();
    }
    if style.underline {
        text = text.underline();
    }
    if let Some([red, green, blue]) = style.color {
        text = text.color(egui::Color32::from_rgb(red, green, blue));
    }
    text
}

fn collapse_whitespace(text: &str) -> String {
    let mut collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if !collapsed.is_empty() && text.starts_with(char::is_whitespace) {
        collapsed.insert(0, ' ');
    }
    if !collapsed.is_empty() && text.ends_with(char::is_whitespace) {
        collapsed.push(' ');
    }
    collapsed
}

fn is_block_node(node: &RenderNode) -> bool {
    node.style.display == Some(dragon_core::DisplayMode::Block)
        || node.style.display == Some(dragon_core::DisplayMode::Flex)
        || (node.style.display.is_none() && node.tag.as_deref().is_some_and(is_block_element))
}

fn is_block_element(tag: &str) -> bool {
    matches!(
        tag,
        "address"
            | "article"
            | "aside"
            | "blockquote"
            | "body"
            | "caption"
            | "dd"
            | "details"
            | "dialog"
            | "div"
            | "dl"
            | "dt"
            | "fieldset"
            | "figcaption"
            | "figure"
            | "footer"
            | "form"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "header"
            | "hr"
            | "li"
            | "main"
            | "nav"
            | "ol"
            | "p"
            | "pre"
            | "section"
            | "table"
            | "tbody"
            | "tfoot"
            | "thead"
            | "tr"
            | "ul"
    )
}

fn decode_page_image(bytes: &[u8]) -> Result<image::DynamicImage, String> {
    let reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|error| error.to_string())?;
    let (width, height) = reader
        .into_dimensions()
        .map_err(|error| error.to_string())?;
    if u64::from(width) * u64::from(height) > MAX_IMAGE_PIXELS || width > 4096 || height > 4096 {
        return Err("kích thước ảnh vượt giới hạn giải mã".into());
    }
    image::load_from_memory(bytes).map_err(|error| error.to_string())
}

fn normalize_address(address: &str) -> String {
    let address = address.trim();
    if address.is_empty() || address.contains("://") {
        address.to_owned()
    } else {
        format!("https://{address}")
    }
}

fn truncate_tab_title(title: &str) -> String {
    const MAX_CHARS: usize = 24;
    if title.chars().count() <= MAX_CHARS {
        return title.to_owned();
    }
    let prefix = title.chars().take(MAX_CHARS - 1).collect::<String>();
    format!("{prefix}…")
}

fn main() -> eframe::Result<()> {
    let initial_url = std::env::args().nth(1);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Dragon Browser")
            .with_inner_size([1000.0, 720.0])
            .with_min_inner_size([640.0, 420.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Dragon Browser",
        options,
        Box::new(move |creation_context| {
            let mut fonts = egui::FontDefinitions::default();
            fonts.font_data.insert(
                "DejaVuSans".to_owned(),
                egui::FontData::from_static(include_bytes!("../assets/DejaVuSans.ttf")).into(),
            );
            for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
                fonts
                    .families
                    .entry(family)
                    .or_default()
                    .push("DejaVuSans".to_owned());
            }
            creation_context.egui_ctx.set_fonts(fonts);
            Ok(Box::new(DragonBrowserApp::new(initial_url)))
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::{normalize_address, resolve_link, truncate_tab_title};

    #[test]
    fn adds_https_to_bare_addresses() {
        assert_eq!(normalize_address("vnexpress.net"), "https://vnexpress.net");
    }

    #[test]
    fn preserves_explicit_schemes_and_trims_whitespace() {
        assert_eq!(
            normalize_address("  http://example.com  "),
            "http://example.com"
        );
    }

    #[test]
    fn truncates_long_tab_labels_without_splitting_utf8() {
        assert_eq!(
            truncate_tab_title("短".repeat(26).as_str()).chars().count(),
            24
        );
        assert_eq!(truncate_tab_title("Dragon"), "Dragon");
    }

    #[test]
    fn resolves_relative_links_and_rejects_non_web_schemes() {
        assert_eq!(
            resolve_link("https://example.com/news/page", "../story?id=2").as_deref(),
            Some("https://example.com/story?id=2")
        );
        assert_eq!(
            resolve_link("https://example.com/", "javascript:alert(1)"),
            None
        );
    }
}
