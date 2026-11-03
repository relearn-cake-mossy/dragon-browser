use std::collections::HashMap;
use std::io::Cursor;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

use dragon_core::{Article, RenderNode, TextAlignment, TextStyle};
use eframe::egui;

const MAX_IMAGE_PIXELS: u64 = 1_000_000;

struct DragonBrowserApp {
    address: String,
    page: Option<Article>,
    page_images: HashMap<String, egui::TextureHandle>,
    logo: Option<egui::TextureHandle>,
    logo_load_attempted: bool,
    error: Option<String>,
    loading: bool,
    request_id: u64,
    sender: Sender<(u64, Result<Article, String>)>,
    receiver: Receiver<(u64, Result<Article, String>)>,
}

impl DragonBrowserApp {
    fn new(initial_url: Option<String>) -> Self {
        let (sender, receiver) = mpsc::channel();
        let mut app = Self {
            address: initial_url.unwrap_or_default(),
            page: None,
            page_images: HashMap::new(),
            logo: None,
            logo_load_attempted: false,
            error: None,
            loading: false,
            request_id: 0,
            sender,
            receiver,
        };
        if !app.address.is_empty() {
            app.navigate();
        }
        app
    }

    fn navigate(&mut self) {
        let address = normalize_address(&self.address);
        if address.is_empty() {
            return;
        }

        self.address = address.clone();
        self.page = None;
        self.page_images.clear();
        self.error = None;
        self.loading = true;
        self.request_id += 1;
        let request_id = self.request_id;
        let sender = self.sender.clone();

        thread::spawn(move || {
            let result = dragon_core::load_url(&address).map_err(|error| error.to_string());
            let _ = sender.send((request_id, result));
        });
    }

    fn receive_page(&mut self, context: &egui::Context) {
        while let Ok((request_id, result)) = self.receiver.try_recv() {
            if request_id != self.request_id {
                continue;
            }
            self.loading = false;
            match result {
                Ok(mut article) => {
                    self.page_images.clear();
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
                                self.page_images.insert(image.url.clone(), texture);
                            }
                            Err(error) => article
                                .resource_warnings
                                .push(format!("Không hiển thị được ảnh {}: {error}", image.url)),
                        }
                    }
                    self.page = Some(article);
                }
                Err(error) => self.error = Some(error),
            }
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
        if self.loading {
            context.request_repaint_after(Duration::from_millis(100));
        }

        egui::TopBottomPanel::top("browser_toolbar").show(context, |ui| {
            ui.horizontal(|ui| {
                if let Some(logo) = &self.logo {
                    ui.add(egui::Image::new(logo).fit_to_exact_size(egui::vec2(30.0, 30.0)));
                }
                ui.heading(
                    egui::RichText::new("Dragon").color(egui::Color32::from_rgb(194, 66, 54)),
                );
                ui.add_space(8.0);
                let address = ui.add_sized(
                    [ui.available_width() - 88.0, 34.0],
                    egui::TextEdit::singleline(&mut self.address)
                        .hint_text("Nhập địa chỉ trang web..."),
                );
                let enter_pressed =
                    address.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
                if ui.button("Đi").clicked() || enter_pressed {
                    self.navigate();
                }
            });
        });

        egui::TopBottomPanel::bottom("status_bar").show(context, |ui| {
            ui.horizontal(|ui| {
                ui.label("Dragon renderer tự viết");
                ui.separator();
                ui.label(if self.loading {
                    "Đang tải trang..."
                } else {
                    "HTML/CSS/JavaScript hỗ trợ giới hạn"
                });
            });
        });

        egui::CentralPanel::default().show(context, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.add_space(20.0);
                    if let Some(error) = &self.error {
                        ui.heading("Không thể mở trang");
                        ui.label(error);
                    } else if let Some(article) = &self.page {
                        for warning in &article.resource_warnings {
                            ui.colored_label(egui::Color32::DARK_RED, warning);
                        }
                        render_node(ui, &article.render_tree, &self.page_images);
                    } else if self.loading {
                        ui.spinner();
                        ui.label("Dragon đang tải nội dung...");
                    } else {
                        ui.vertical_centered(|ui| {
                            ui.add_space(100.0);
                            ui.heading(
                                egui::RichText::new("Chào mừng đến với Dragon Browser").size(26.0),
                            );
                            ui.add_space(8.0);
                            ui.label("Nhập địa chỉ trang web ở thanh phía trên để bắt đầu.");
                        });
                    }
                });
        });
    }
}

fn render_node(
    ui: &mut egui::Ui,
    node: &RenderNode,
    images: &HashMap<String, egui::TextureHandle>,
) {
    if node.tag.is_none() {
        render_inline(ui, node, images);
        return;
    }

    let tag = node.tag.as_deref().unwrap_or_default();
    if tag == "img" {
        render_image(ui, node, images);
        return;
    }
    if !is_block_element(tag) {
        render_inline(ui, node, images);
        return;
    }

    let spacing = match tag {
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => 12.0,
        "p" | "blockquote" | "pre" | "ul" | "ol" | "table" | "figure" => 10.0,
        _ => 0.0,
    };
    if spacing > 0.0 {
        ui.add_space(spacing);
    }
    let draw_contents = |ui: &mut egui::Ui| {
        if tag == "li" {
            ui.horizontal_wrapped(|ui| {
                ui.label("• ");
                render_flow_children(ui, node, images);
            });
        } else {
            render_flow_children(ui, node, images);
        }
    };
    let draw_aligned = |ui: &mut egui::Ui| match node.style.text_align {
        Some(TextAlignment::Center) => {
            ui.with_layout(egui::Layout::top_down(egui::Align::Center), draw_contents);
        }
        Some(TextAlignment::Right) => {
            ui.with_layout(egui::Layout::top_down(egui::Align::RIGHT), draw_contents);
        }
        _ => draw_contents(ui),
    };
    if let Some([red, green, blue]) = node.style.background_color {
        egui::Frame::new()
            .fill(egui::Color32::from_rgb(red, green, blue))
            .inner_margin(egui::Margin::symmetric(8, 4))
            .show(ui, draw_aligned);
    } else {
        draw_aligned(ui);
    }
    if spacing > 0.0 {
        ui.add_space(spacing);
    }
}

fn render_flow_children(
    ui: &mut egui::Ui,
    node: &RenderNode,
    images: &HashMap<String, egui::TextureHandle>,
) {
    let mut index = 0;
    while index < node.children.len() {
        if is_block_node(&node.children[index]) {
            render_node(ui, &node.children[index], images);
            index += 1;
            continue;
        }

        let start = index;
        while index < node.children.len() && !is_block_node(&node.children[index]) {
            index += 1;
        }
        ui.horizontal_wrapped(|ui| {
            for child in &node.children[start..index] {
                render_inline(ui, child, images);
            }
        });
    }
}

fn render_inline(
    ui: &mut egui::Ui,
    node: &RenderNode,
    images: &HashMap<String, egui::TextureHandle>,
) {
    if node.image_url.is_some() {
        render_image(ui, node, images);
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
                render_inline_children(ui, node, images);
            });
    } else {
        render_inline_children(ui, node, images);
    }
}

fn render_inline_children(
    ui: &mut egui::Ui,
    node: &RenderNode,
    images: &HashMap<String, egui::TextureHandle>,
) {
    for child in &node.children {
        render_inline(ui, child, images);
    }
}

fn render_image(
    ui: &mut egui::Ui,
    node: &RenderNode,
    images: &HashMap<String, egui::TextureHandle>,
) {
    if let Some(image_url) = &node.image_url {
        if let Some(texture) = images.get(image_url) {
            ui.add(egui::Image::new(texture).max_width(ui.available_width()));
        } else if !node.image_alt.is_empty() {
            ui.label(&node.image_alt);
        }
    }
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
    node.tag.as_deref().is_some_and(is_block_element)
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
        Box::new(move |_creation_context| Ok(Box::new(DragonBrowserApp::new(initial_url)))),
    )
}

#[cfg(test)]
mod tests {
    use super::normalize_address;

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
}
