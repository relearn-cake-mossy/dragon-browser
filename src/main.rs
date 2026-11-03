use std::collections::HashMap;
use std::io::Cursor;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

use dragon_core::{Article, TextAlignment};
use eframe::egui;

const MAX_IMAGE_PIXELS: u64 = 1_000_000;

struct DragonBrowserApp {
    address: String,
    page: Option<Article>,
    page_images: HashMap<String, egui::TextureHandle>,
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
        self.receive_page(context);
        if self.loading {
            context.request_repaint_after(Duration::from_millis(100));
        }

        egui::TopBottomPanel::top("browser_toolbar").show(context, |ui| {
            ui.horizontal(|ui| {
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
                ui.label("Dragon Core");
                ui.separator();
                ui.label(if self.loading {
                    "Đang tải trang..."
                } else {
                    "Dragon Core · HTML/CSS/JavaScript cơ bản; chưa tương thích đầy đủ"
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
                        if !article.title.is_empty() {
                            ui.heading(&article.title);
                            ui.add_space(14.0);
                        }
                        for block in &article.blocks {
                            if let Some(image_url) = &block.image_url {
                                if let Some(texture) = self.page_images.get(image_url) {
                                    ui.add(
                                        egui::Image::new(texture).max_width(ui.available_width()),
                                    );
                                } else if !block.image_alt.is_empty() {
                                    ui.label(&block.image_alt);
                                }
                                continue;
                            }
                            if block.heading_level.is_some() && block.text == article.title {
                                continue;
                            }
                            let default_size = match block.heading_level {
                                Some(1) => 26.0,
                                Some(2) => 22.0,
                                Some(3) => 19.0,
                                Some(_) => 17.0,
                                None => 17.0,
                            };
                            let mut text = egui::RichText::new(&block.text)
                                .size(block.style.font_size.unwrap_or(default_size))
                                .line_height(Some(26.0));
                            if block.style.bold.unwrap_or(block.heading_level.is_some())
                                || block.heading_level.is_some()
                            {
                                text = text.strong();
                            }
                            if let Some([red, green, blue]) = block.style.color {
                                text = text.color(egui::Color32::from_rgb(red, green, blue));
                            }
                            let add_text = |ui: &mut egui::Ui| {
                                let text = egui::Label::new(text.clone()).wrap();
                                if let Some([red, green, blue]) = block.style.background_color {
                                    egui::Frame::new()
                                        .fill(egui::Color32::from_rgb(red, green, blue))
                                        .inner_margin(egui::Margin::symmetric(8, 4))
                                        .show(ui, |ui| {
                                            ui.add(text);
                                        });
                                } else {
                                    ui.add(text);
                                }
                            };
                            match block.style.text_align {
                                Some(TextAlignment::Center) => {
                                    ui.with_layout(
                                        egui::Layout::top_down(egui::Align::Center),
                                        add_text,
                                    );
                                }
                                Some(TextAlignment::Right) => {
                                    ui.with_layout(
                                        egui::Layout::top_down(egui::Align::RIGHT),
                                        add_text,
                                    );
                                }
                                _ => add_text(ui),
                            }
                            ui.add_space(if block.heading_level.is_some() {
                                12.0
                            } else {
                                8.0
                            });
                        }
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
    if address.is_empty() || address.starts_with("http://") || address.starts_with("https://") {
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
