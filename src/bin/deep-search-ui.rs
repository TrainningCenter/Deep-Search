use deep_search::client::{CompleteSearch, ServiceClient};
use eframe::egui;
use egui_extras::{Column, TableBuilder};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

enum UiMessage {
    Status(Value),
    Indexed(Value),
    Search(CompleteSearch),
    Error(String),
}

#[derive(Clone, Serialize, Deserialize)]
struct ColumnSetting {
    key: String,
    visible: bool,
    width: f32,
}

#[derive(Clone, Serialize, Deserialize)]
struct ViewProfile {
    columns: Vec<ColumnSetting>,
    row_height: f32,
}

#[derive(Default, Serialize, Deserialize)]
struct UiSettings {
    profiles: BTreeMap<String, ViewProfile>,
    active_profile: String,
}

fn default_columns() -> Vec<ColumnSetting> {
    vec![
        ColumnSetting { key: "name".into(), visible: true, width: 260.0 },
        ColumnSetting { key: "extension".into(), visible: true, width: 90.0 },
        ColumnSetting { key: "size".into(), visible: true, width: 110.0 },
        ColumnSetting { key: "modified".into(), visible: false, width: 160.0 },
        ColumnSetting { key: "path".into(), visible: true, width: 420.0 },
    ]
}

fn default_profile(columns: &[&str]) -> ViewProfile {
    let mut settings = default_columns();
    for column in &mut settings {
        column.visible = columns.contains(&column.key.as_str());
    }
    ViewProfile { columns: settings, row_height: 25.0 }
}

fn settings_path() -> PathBuf {
    #[cfg(target_os = "windows")]
    let base = std::env::var_os("APPDATA").map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    #[cfg(target_os = "macos")]
    let base = std::env::var_os("HOME").map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
        .join("Library").join("Application Support");
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let base = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    base.join("deep-search").join("ui-settings.json")
}

impl UiSettings {
    fn load() -> Self {
        let mut settings = std::fs::read(settings_path())
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Self>(&bytes).ok())
            .unwrap_or_default();
        if settings.profiles.is_empty() {
            settings.profiles.insert("Default".into(), default_profile(&["name", "extension", "size", "path"]));
            settings.profiles.insert("Videos".into(), default_profile(&["name", "extension", "size", "modified", "path"]));
            settings.profiles.insert("Documents".into(), default_profile(&["name", "extension", "modified", "size", "path"]));
            settings.profiles.insert("Large files".into(), default_profile(&["name", "size", "extension", "path"]));
            settings.profiles.insert("NAS".into(), default_profile(&["name", "size", "modified", "path"]));
            settings.active_profile = "Default".into();
            settings.save();
        }
        if !settings.profiles.contains_key(&settings.active_profile) {
            settings.active_profile = settings.profiles.keys().next().cloned().unwrap_or_default();
        }
        settings
    }

    fn save(&self) {
        let path = settings_path();
        if let Some(parent) = path.parent() {
            if std::fs::create_dir_all(parent).is_err() { return; }
        }
        if let Ok(bytes) = serde_json::to_vec_pretty(self) {
            let _ = std::fs::write(path, bytes);
        }
    }
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
    settings: UiSettings,
    new_profile_name: String,
    arabic: bool,
}

impl Default for DeepSearchApp {
    fn default() -> Self {
        let (sender, receiver) = mpsc::channel();
        let settings = UiSettings::load();
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
            settings,
            new_profile_name: String::new(),
            arabic: false,
        }
    }
}

impl DeepSearchApp {
    fn client(&self) -> ServiceClient {
        ServiceClient::new(self.service_address.trim().to_owned())
    }

    fn active_profile(&self) -> &ViewProfile {
        self.settings.profiles.get(&self.settings.active_profile)
            .expect("an active view profile is always initialized")
    }

    fn active_profile_mut(&mut self) -> &mut ViewProfile {
        self.settings.profiles.get_mut(&self.settings.active_profile)
            .expect("an active view profile is always initialized")
    }

    fn persist_settings(&self) {
        self.settings.save();
    }

    fn activate_profile(&mut self, name: String) {
        if self.settings.profiles.contains_key(&name) {
            self.settings.active_profile = name;
            self.persist_settings();
        }
    }

    fn save_profile_as(&mut self) {
        let name = self.new_profile_name.trim().to_owned();
        if name.is_empty() { return; }
        let profile = self.active_profile().clone();
        self.settings.profiles.insert(name.clone(), profile);
        self.settings.active_profile = name;
        self.new_profile_name.clear();
        self.persist_settings();
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
            .map(str::trim).filter(|path| !path.is_empty())
            .map(str::to_owned).collect();
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

    fn render_column_controls(&mut self, ui: &mut egui::Ui) {
        ui.heading(if self.arabic { "إعدادات الأعمدة" } else { "Columns & layout" });
        let mut changed = false;
        let mut move_request: Option<(usize, isize)> = None;
        let columns = &mut self.active_profile_mut().columns;
        let column_count = columns.len();
        for index in 0..column_count {
            ui.horizontal(|ui| {
                let column = &mut columns[index];
                changed |= ui.checkbox(&mut column.visible, column.key.as_str()).changed();
                ui.add(egui::DragValue::new(&mut column.width).range(60.0..=1200.0).speed(5.0).suffix(" px"));
                if ui.small_button("↑").clicked() && index > 0 {
                    move_request = Some((index, -1));
                }
                if ui.small_button("↓").clicked() && index + 1 < columns.len() {
                    move_request = Some((index, 1));
                }
            });
        }
        if let Some((index, direction)) = move_request {
            let target = (index as isize + direction) as usize;
            columns.swap(index, target);
            changed = true;
        }
        let row_height = &mut self.active_profile_mut().row_height;
        ui.horizontal(|ui| {
            ui.label(if self.arabic { "ارتفاع الصف" } else { "Row height" });
            changed |= ui.add(egui::Slider::new(row_height, 18.0..=60.0).suffix(" px")).changed();
        });
        if ui.button(if self.arabic { "ملاءمة تلقائية للأعمدة" } else { "Auto-fit columns" }).clicked() {
            for column in &mut self.active_profile_mut().columns {
                column.width = match column.key.as_str() {
                    "name" => 260.0, "extension" => 100.0, "size" => 120.0,
                    "modified" => 180.0, "path" => 520.0, _ => 160.0,
                };
            }
            changed = true;
        }
        if changed { self.persist_settings(); }
    }

    fn render_results(&mut self, ui: &mut egui::Ui) {
        ui.label(format!("{} result(s)", self.total));
        let columns = self.active_profile().columns.iter()
            .filter(|column| column.visible).cloned().collect::<Vec<_>>();
        if columns.is_empty() {
            ui.centered_and_justified(|ui| ui.label("Show at least one column in Columns & layout."));
            return;
        }
        let row_height = self.active_profile().row_height;
        let mut table = TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center));
        for column in &columns {
            table = table.column(Column::initial(column.width).at_least(60.0));
        }
        table.header(28.0, |mut header| {
            for column in &columns {
                header.col(|ui| { ui.strong(column.key.to_uppercase()); });
            }
        }).body(|body| {
            body.rows(row_height, self.results.len(), |mut row| {
                let result = &self.results[row.index()];
                for column in &columns {
                    row.col(|ui| {
                        match column.key.as_str() {
                            "name" | "extension" | "path" => {
                                let value = result.get(&column.key).and_then(Value::as_str).unwrap_or("—");
                                ui.add(egui::Label::new(value).truncate());
                            }
                            "modified" => {
                                let value = result.get("modified_timestamp_ns").and_then(Value::as_u64)
                                    .map(format_modified).unwrap_or_else(|| "—".to_owned());
                                ui.label(value);
                            }
                            "size" => {
                                let size = result.get("size_bytes").and_then(Value::as_u64).unwrap_or(0);
                                ui.label(format_size(size));
                            }
                            _ => { ui.label("—"); }
                        }
                    });
                }
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
                ui.separator();
                let profiles = self.settings.profiles.keys().cloned().collect::<Vec<_>>();
                let active = self.settings.active_profile.clone();
                egui::ComboBox::from_id_salt("view_profiles")
                    .selected_text(&active)
                    .show_ui(ui, |ui| {
                        for profile in profiles {
                            if ui.selectable_label(profile == active, &profile).clicked() {
                                self.activate_profile(profile);
                            }
                        }
                    });
                if ui.button("Save profile").clicked() { self.persist_settings(); }
                ui.add(egui::TextEdit::singleline(&mut self.new_profile_name).hint_text("New profile name").desired_width(130.0));
                if ui.button("Save as").clicked() { self.save_profile_as(); }
                if ui.button(if self.arabic { "English" } else { "العربية" }).clicked() {
                    self.arabic = !self.arabic;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let Some(count) = self.indexed_files { ui.label(format!("{count} indexed files")); }
                });
            });
        });

        egui::SidePanel::left("sources").resizable(true).default_width(290.0).show(ctx, |ui| {
            ui.heading(if self.arabic { "الخدمة" } else { "Service" });
            ui.label(if self.arabic { "عنوان الخدمة" } else { "Address" });
            ui.text_edit_singleline(&mut self.service_address);
            if ui.button(if self.arabic { "اتصال / تحديث الحالة" } else { "Connect / Refresh status" }).clicked() {
                self.check_status();
            }
            ui.separator();
            ui.heading(if self.arabic { "مجلدات الفهرسة" } else { "Index locations" });
            ui.label(if self.arabic { "مسار مجلد في كل سطر:" } else { "One folder path per line:" });
            ui.add(egui::TextEdit::multiline(&mut self.roots).desired_rows(4).desired_width(f32::INFINITY));
            if ui.add_enabled(!self.busy, egui::Button::new(if self.arabic { "ابدأ الفهرسة" } else { "Index locations" })).clicked() {
                self.index_roots();
            }
            ui.separator();
            ui.heading(if self.arabic { "بحث" } else { "Search" });
            ui.text_edit_singleline(&mut self.search_query);
            egui::ComboBox::from_label(if self.arabic { "ترتيب حسب" } else { "Sort by" })
                .selected_text(&self.sort_field).show_ui(ui, |ui| {
                    for field in ["name", "path", "extension", "size", "modified", "relevance"] {
                        ui.selectable_value(&mut self.sort_field, field.to_owned(), field);
                    }
                });
            ui.checkbox(&mut self.descending, if self.arabic { "ترتيب تنازلي" } else { "Descending" });
            if ui.add_enabled(!self.busy, egui::Button::new(if self.arabic { "بحث في كل النتائج" } else { "Search all results" })).clicked() {
                self.search();
            }
            ui.separator();
            if self.busy { ui.spinner(); }
            ui.label(&self.message);
        });

        egui::SidePanel::right("view_settings").resizable(true).default_width(270.0).show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| self.render_column_controls(ui));
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading(if self.arabic { "نتائج البحث" } else { "Search results" });
            ui.label(if self.arabic {
                "جدول واحد مستمر · يتم جلب جميع النتائج تلقائيًا · يمكن تغيير عرض الأعمدة"
            } else {
                "One continuous table · all matches are fetched automatically · resize columns by dragging"
            });
            ui.separator();
            egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| self.render_results(ui));
        });

        if self.busy { ctx.request_repaint_after(std::time::Duration::from_millis(100)); }
    }
}

fn format_modified(timestamp_ns: u64) -> String {
    let seconds = (timestamp_ns / 1_000_000_000) as i64;
    let days = seconds.div_euclid(86_400);
    let day_seconds = seconds.rem_euclid(86_400);
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era = (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    if month <= 2 { year += 1; }
    let hour = day_seconds / 3_600;
    let minute = (day_seconds % 3_600) / 60;
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02} UTC")
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
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([1000.0, 650.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Deep-Search",
        options,
        Box::new(|_creation_context| {
            let mut app = DeepSearchApp::default();
            app.check_status();
            Ok(Box::new(app))
        }),
    )
}
