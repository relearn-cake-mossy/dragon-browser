use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

use dragon_core::Article;
use eframe::egui;

struct DragonBrowserApp {
    address: String,
    page: Option<Article>,
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

    fn receive_page(&mut self) {
        while let Ok((request_id, result)) = self.receiver.try_recv() {
            if request_id != self.request_id {
                continue;
            }
            self.loading = false;
            match result {
                Ok(article) => self.page = Some(article),
                Err(error) => self.error = Some(error),
            }
        }
    }
}

impl eframe::App for DragonBrowserApp {
    fn update(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        self.receive_page();
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
                    "Chế độ đọc thử nghiệm · chưa hỗ trợ JavaScript/DOM trên trang"
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
                        if !article.title.is_empty() {
                            ui.heading(&article.title);
                            ui.add_space(14.0);
                        }
                        for block in &article.blocks {
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
                            if block.style.bold.unwrap_or(block.heading_level.is_some()) {
                                text = text.strong();
                            }
                            if let Some([red, green, blue]) = block.style.color {
                                text = text.color(egui::Color32::from_rgb(red, green, blue));
                            }
                            ui.add(egui::Label::new(text).wrap());
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
