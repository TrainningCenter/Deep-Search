use deep_search::client::{CompleteSearch, ServiceClient};
use eframe::egui;
use egui_extras::{Column, TableBuilder};
use serde_json::Value;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

enum UiMessage {
    Status(Value),
    Indexed(Value),
    Search(CompleteSearch),
    Error(String),
}

struct DeepSearchApp {
    service_address: String,
    search_query: String,
    roots: String,
    sort_field: String,
    descending: bool,
    results: Vec<Value>,
    total: u64,
    indexed_files: Option<u64>,
    message: String,
    busy: bool,
    sender: Sender<UiMessage>,
    receiver: Receiver<UiMessage>,
}

impl Default for DeepSearchApp {
    fn default() -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            service_address: "127.0.0.1:4765".to_owned(),
            search_query: "type:video".to_owned(),
            roots: String::new(),
            sort_field: "name".to_owned(),
            descending: false,
            results: Vec::new(),
            total: 0,
            indexed_files: None,
            message: "Connect to the Deep-Search background service to begin.".to_owned(),
            busy: false,
            sender,
            receiver,
        }
    }
}

impl DeepSearchApp {
    fn client(&self) -> ServiceClient {
        ServiceClient::new(self.service_address.trim().to_owned())
    }

    fn check_status(&mut self) {
        let client = self.client();
        let sender = self.sender.clone();
        self.busy = true;
        self.message = "Connecting to service…".to_owned();
        thread::spawn(move || {
            let result = client.status().map(UiMessage::Status)
                .unwrap_or_else(|error| UiMessage::Error(error.to_string()));
            let _ = sender.send(result);
        });
    }

    fn index_roots(&mut self) {
        let paths: Vec<String> = self.roots.lines()
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .map(str::to_owned)
            .collect();
        if paths.is_empty() {
            self.message = "Enter at least one folder path (one path per line).".to_owned();
            return;
        }
        let client = self.client();
        let sender = self.sender.clone();
        self.busy = true;
        self.message = format!("Indexing {} location(s)…", paths.len());
        thread::spawn(move || {
            let result = client.index(&paths).map(UiMessage::Indexed)
                .unwrap_or_else(|error| UiMessage::Error(error.to_string()));
            let _ = sender.send(result);
        });
    }

    fn search(&mut self) {
        let query = self.search_query.trim().to_owned();
        if query.is_empty() {
            self.message = "Enter a search query.".to_owned();
            return;
        }
        let client = self.client();
        let sender = self.sender.clone();
        let sort = self.sort_field.clone();
        let order = if self.descending { "desc" } else { "asc" }.to_owned();
        self.busy = true;
        self.message = "Searching and retrieving every matching result…".to_owned();
        self.results.clear();
        self.total = 0;
        thread::spawn(move || {
            let result = client.search_all(&query, &sort, &order)
                .map(UiMessage::Search)
                .unwrap_or_else(|error| UiMessage::Error(error.to_string()));
            let _ = sender.send(result);
        });
    }

    fn receive_messages(&mut self) {
        while let Ok(message) = self.receiver.try_recv() {
            self.busy = false;
            match message {
                UiMessage::Status(status) => {
                    self.indexed_files = status.get("indexed_files").and_then(Value::as_u64);
                    self.message = format!(
                        "Service connected · version {}",
                        status.get("version").and_then(Value::as_str).unwrap_or("unknown")
                    );
                }
                UiMessage::Indexed(report) => {
                    let saved = report.get("files_saved").and_then(Value::as_u64).unwrap_or(0);
                    let discovered = report.get("files_discovered").and_then(Value::as_u64).unwrap_or(0);
                    let errors = report.get("errors").and_then(Value::as_u64).unwrap_or(0);
                    self.message = format!(
                        "Indexing finished · {saved} saved · {discovered} discovered · {errors} errors"
                    );
                    self.check_status();
                }
                UiMessage::Search(search) => {
                    self.total = search.total;
                    self.results = search.results;
                    self.message = format!("Complete · {} of {} results loaded", self.results.len(), self.total);
                }
                UiMessage::Error(error) => {
                    self.message = format!("Error: {error}");
                }
            }
        }
    }

    fn result_table(&mut self, ui: &mut egui::Ui) {
        ui.label(format!("{} result(s)", self.total));
        TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::initial(260.0).at_least(100.0))
            .column(Column::initial(90.0).at_least(60.0))
            .column(Column::initial(110.0).at_least(70.0))
            .column(Column::remainder().at_least(220.0))
            .header(26.0, |mut header| {
                header.col(|ui| { ui.strong("Name"); });
                header.col(|ui| { ui.strong("Extension"); });
                header.col(|ui| { ui.strong("Size"); });
                header.col(|ui| { ui.strong("Path"); });
            })
            .body(|body| {
                body.rows(25.0, self.results.len(), |mut row| {
                    let result = &self.results[row.index()];
                    row.col(|ui| {
                        ui.label(result.get("name").and_then(Value::as_str).unwrap_or("—"));
                    });
                    row.col(|ui| {
                        ui.label(result.get("extension").and_then(Value::as_str).unwrap_or("—"));
                    });
                    row.col(|ui| {
                        let size = result.get("size_bytes").and_then(Value::as_u64).unwrap_or(0);
                        ui.label(format_size(size));
                    });
                    row.col(|ui| {
                        ui.add(egui::Label::new(
                            result.get("path").and_then(Value::as_str).unwrap_or("—")
                        ).truncate());
                    });
                });
            });
    }
}

impl eframe::App for DeepSearchApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.receive_messages();
        egui::TopBottomPanel::top("top_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Deep-Search");
                ui.separator();
                ui.label("Desktop Alpha");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let Some(count) = self.indexed_files {
                        ui.label(format!("{count} indexed files"));
                    }
                });
            });
        });

        egui::SidePanel::left("sources")
            .resizable(true)
            .default_width(270.0)
            .show(ctx, |ui| {
                ui.heading("Service");
                ui.label("Address");
                ui.text_edit_singleline(&mut self.service_address);
                if ui.button("Connect / Refresh status").clicked() {
                    self.check_status();
                }
                ui.separator();
                ui.heading("Index locations");
                ui.label("Enter one folder path per line:");
                ui.add(
                    egui::TextEdit::multiline(&mut self.roots)
                        .desired_rows(5)
                        .desired_width(f32::INFINITY)
                );
                if ui.add_enabled(!self.busy, egui::Button::new("Index locations")).clicked() {
                    self.index_roots();
                }
                ui.separator();
                ui.heading("Search");
                ui.label("Query");
                ui.text_edit_singleline(&mut self.search_query);
                egui::ComboBox::from_label("Sort by")
                    .selected_text(&self.sort_field)
                    .show_ui(ui, |ui| {
                        for field in ["name", "path", "extension", "size", "modified", "relevance"] {
                            ui.selectable_value(&mut self.sort_field, field.to_owned(), field);
                        }
                    });
                ui.checkbox(&mut self.descending, "Descending");
                if ui.add_enabled(!self.busy, egui::Button::new("Search all results")).clicked() {
                    self.search();
                }
                ui.separator();
                if self.busy {
                    ui.spinner();
                }
                ui.label(&self.message);
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Search results");
            ui.label("One continuous table · all matches are fetched automatically · resize columns by dragging");
            ui.separator();
            egui::ScrollArea::both()
                .auto_shrink([false, false])
                .show(ui, |ui| self.result_table(ui));
        });

        if self.busy {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }
}

fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 { format!("{bytes} B") } else { format!("{size:.1} {}", UNITS[unit]) }
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([900.0, 600.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Deep-Search",
        options,
        Box::new(|_creation_context| {
            let mut app = DeepSearchApp::default();
            // Try the local background service immediately when the window opens.
            // The user can still change the address and reconnect manually.
            app.check_status();
            Ok(Box::new(app))
        }),
    )
}
