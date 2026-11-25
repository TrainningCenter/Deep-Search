use deep_search::client::{CompleteSearch, ServiceClient};
use eframe::egui;
use egui_table::{AutoSizeMode, CellInfo, Column as TableColumn, HeaderCellInfo, HeaderRow, Table, TableDelegate, TableState};
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
    ServiceState { active: Option<bool>, enabled: Option<bool>, detail: String },
    ServiceAction { action: String, success: bool, detail: String },
    PickedPaths(Vec<String>),
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
    #[serde(default = "default_frozen_columns")]
    frozen_columns: usize,
}

fn default_frozen_columns() -> usize { 1 }

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
    ViewProfile { columns: settings, row_height: 25.0, frozen_columns: 1 }
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
    service_active: Option<bool>,
    service_enabled: Option<bool>,
    service_detail: String,
    service_busy: bool,
    service_checked: bool,
    dragged_column: Option<usize>,
    reset_table_layout: bool,
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
            service_active: None,
            service_enabled: None,
            service_detail: "Checking system service status…".to_owned(),
            service_busy: false,
            service_checked: false,
            dragged_column: None,
            reset_table_layout: false,
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

    fn check_system_service(&mut self) {
        let sender = self.sender.clone();
        let username = std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME"))
            .unwrap_or_else(|_| "unknown".to_owned());
        self.service_busy = true;
        thread::spawn(move || {
            let active = service_helper("is-active", &username, false);
            let enabled = service_helper("is-enabled", &username, false);
            let detail = match (&active, &enabled) {
                (Ok(a), Ok(e)) => format!("service · active: {a} · boot startup: {e}"),
                (Err(error), _) | (_, Err(error)) => error.clone(),
            };
            let _ = sender.send(UiMessage::ServiceState {
                active: active.ok().map(|value| value.trim() == "active"),
                enabled: enabled.ok().map(|value| value.trim() == "enabled"),
                detail,
            });
        });
    }

    fn control_system_service(&mut self, action: &str) {
        let action = action.to_owned();
        let sender = self.sender.clone();
        let username = std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME"))
            .unwrap_or_else(|_| "unknown".to_owned());
        self.service_busy = true;
        self.service_detail = format!("Requesting system service action: {action}…");
        thread::spawn(move || {
            let result = service_helper(&action, &username, true);
            let (success, detail) = match result {
                Ok(output) => (true, output),
                Err(error) => (false, error),
            };
            let _ = sender.send(UiMessage::ServiceAction { action, success, detail });
        });
    }

    fn pick_folder(&mut self) {
        let sender = self.sender.clone();
        self.busy = true;
        self.message = if self.arabic {
            "جارٍ فتح نافذة اختيار المجلد…"
        } else {
            "Opening folder picker…"
        }.to_owned();
        thread::spawn(move || {
            let paths = native_folder_picker();
            let _ = sender.send(UiMessage::PickedPaths(paths));
        });
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
                UiMessage::ServiceState { active, enabled, detail } => {
                    self.service_active = active;
                    self.service_enabled = enabled;
                    self.service_detail = detail;
                    self.service_busy = false;
                }
                UiMessage::ServiceAction { action, success, detail } => {
                    self.service_detail = if success {
                        format!("{action} completed: {detail}")
                    } else {
                        format!("{action} failed: {detail}")
                    };
                    self.service_busy = false;
                    if success { self.check_system_service(); }
                }
                UiMessage::PickedPaths(paths) => {
                    for path in paths {
                        if !self.roots.lines().any(|existing| existing.trim() == path) {
                            if !self.roots.is_empty() && !self.roots.ends_with('\n') {
                                self.roots.push('\n');
                            }
                            self.roots.push_str(&path);
                        }
                    }
                    self.message = if self.roots.is_empty() {
                        "Folder selection cancelled.".to_owned()
                    } else {
                        "Folder path added to indexing locations.".to_owned()
                    };
                }
            }
        }
    }

    fn render_column_controls(&mut self, ui: &mut egui::Ui) {
        ui.heading(if self.arabic { "إعدادات الأعمدة" } else { "Columns & layout" });
        let mut changed = false;
        let mut move_request: Option<(usize, isize)> = None;
        let mut remove_request: Option<usize> = None;
        let mut drag_started: Option<usize> = None;
        let mut hovered_target: Option<usize> = None;
        let pointer_released = ui.input(|input| input.pointer.any_released());
        let mut next_dragged_column = self.dragged_column;
        let is_dragging = next_dragged_column.is_some();
        let arabic = self.arabic;
        let columns = &mut self.active_profile_mut().columns;
        let column_count = columns.len();
        for index in 0..column_count {
            let row = ui.horizontal(|ui| {
                let handle = ui.add(egui::Label::new("⠿").sense(egui::Sense::drag()));
                if handle.drag_started() { drag_started = Some(index); }
                let column = &mut columns[index];
                let label = column_label(&column.key, arabic);
                changed |= ui.checkbox(&mut column.visible, label).changed();
                // Column widths are resized directly from the table header.
                if ui.small_button("↑").clicked() && index > 0 {
                    move_request = Some((index, -1));
                }
                if ui.small_button("↓").clicked() && index + 1 < column_count {
                    move_request = Some((index, 1));
                }
                if ui.small_button("×").clicked() {
                    remove_request = Some(index);
                }
            });
            if is_dragging && row.response.hovered() { hovered_target = Some(index); }
        }
        if let Some(index) = drag_started { next_dragged_column = Some(index); }
        if pointer_released {
            if let (Some(source), Some(target)) = (next_dragged_column.take(), hovered_target) {
                if source != target && source < columns.len() {
                    let moved = columns.remove(source);
                    let insertion_index = if source < target { target - 1 } else { target };
                    columns.insert(insertion_index.min(columns.len()), moved);
                    changed = true;
                }
            } else {
                next_dragged_column = None;
            }
        }
        if let Some((index, direction)) = move_request {
            let target = (index as isize + direction) as usize;
            columns.swap(index, target);
            changed = true;
        }
        if let Some(index) = remove_request {
            columns.remove(index);
            changed = true;
        }
        if ui.button(if arabic { "إضافة الأعمدة المتاحة" } else { "Add available columns" }).clicked() {
            for candidate in default_columns() {
                if !columns.iter().any(|existing| existing.key == candidate.key) {
                    columns.push(candidate);
                    changed = true;
                }
            }
        }
        self.dragged_column = next_dragged_column;
        let row_height = &mut self.active_profile_mut().row_height;
        ui.horizontal(|ui| {
            ui.label(if arabic { "ارتفاع الصف" } else { "Row height" });
            changed |= ui.add(egui::Slider::new(row_height, 18.0..=60.0).suffix(" px")).changed();
        });
        let visible_count = self.active_profile().columns.iter().filter(|column| column.visible).count();
        let max_frozen = visible_count.min(4);
        let frozen_columns = &mut self.active_profile_mut().frozen_columns;
        ui.horizontal(|ui| {
            ui.label(if arabic { "تثبيت الأعمدة الأولى" } else { "Freeze leading columns" });
            changed |= ui.add(egui::Slider::new(frozen_columns, 0..=max_frozen)).changed();
        });
        if *frozen_columns > max_frozen {
            *frozen_columns = max_frozen;
            changed = true;
        }
        if ui.button(if self.arabic { "ملاءمة تلقائية للأعمدة" } else { "Auto-fit columns" }).clicked() {
            let widths = self.active_profile().columns.iter().map(|column| {
                let header_width = column_label(&column.key, arabic).chars().count();
                let content_width = self.results.iter().take(500)
                    .map(|result| result_column_text(result, &column.key).chars().count())
                    .max().unwrap_or(0);
                let width = (header_width.max(content_width) as f32 * 8.0 + 28.0).clamp(60.0, 1000.0);
                (column.key.clone(), width)
            }).collect::<BTreeMap<_, _>>();
            for column in &mut self.active_profile_mut().columns {
                if let Some(width) = widths.get(&column.key) { column.width = *width; }
            }
            self.reset_table_layout = true;
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
        let frozen_columns = self.active_profile().frozen_columns.min(columns.len());
        let table_columns = columns.iter().map(|column| {
            TableColumn::new(column.width)
                .range(egui::Rangef::new(60.0, 1200.0))
                .id(egui::Id::new(column.key.clone()))
                .resizable(true)
        }).collect::<Vec<_>>();
        let table = Table::new()
            .id_salt(self.settings.active_profile.clone())
            .num_rows(self.results.len() as u64)
            .columns(table_columns.clone())
            .num_sticky_cols(frozen_columns)
            .headers(vec![HeaderRow::new(28.0)])
            .auto_size_mode(AutoSizeMode::Never);
        let table_id = table.get_id(ui);

        if self.reset_table_layout {
            TableState::reset(ui.ctx(), table_id);
        }
        let mut table_state = TableState::load(ui.ctx(), table_id).unwrap_or_default();
        for (index, column) in table_columns.iter().enumerate() {
            let column_id = column.id_for(index);
            if table_state.col_widths.get(&column_id).is_none() {
                table_state.col_widths.insert(column_id, column.current);
            }
        }
        table_state.store(ui.ctx(), table_id);

        let mut delegate = ResultsTableDelegate {
            results: &self.results,
            columns: &columns,
            arabic: self.arabic,
            row_height,
        };
        table.show(ui, &mut delegate);
        self.reset_table_layout = false;

        let updated_state = TableState::load(ui.ctx(), table_id);
        let mut widths_changed = false;
        if let Some(state) = updated_state {
            let profile = self.active_profile_mut();
            for (index, column) in columns.iter().enumerate() {
                let column_id = table_columns[index].id_for(index);
                if let Some(width) = state.col_widths.get(&column_id) {
                    if let Some(setting) = profile.columns.iter_mut().find(|item| item.key == column.key) {
                        if (setting.width - *width).abs() > 0.5 {
                            setting.width = *width;
                            widths_changed = true;
                        }
                    }
                }
            }
        }
        if widths_changed {
            self.persist_settings();
        }
    }
}


struct ResultsTableDelegate<'a> {
    results: &'a [Value],
    columns: &'a [ColumnSetting],
    arabic: bool,
    row_height: f32,
}

impl TableDelegate for ResultsTableDelegate<'_> {
    fn header_cell_ui(&mut self, ui: &mut egui::Ui, cell: &HeaderCellInfo) {
        if let Some(column) = self.columns.get(cell.col_range.start) {
            ui.strong(column_label(&column.key, self.arabic));
        }
    }

    fn cell_ui(&mut self, ui: &mut egui::Ui, cell: &CellInfo) {
        let Some(column) = self.columns.get(cell.col_nr) else { return; };
        let Some(result) = self.results.get(cell.row_nr as usize) else { return; };
        if cell.row_nr % 2 == 1 {
            ui.painter().rect_filled(ui.max_rect(), 0.0, ui.visuals().faint_bg_color);
        }
        let value = result_column_text(result, &column.key);
        if column.key == "name" {
            let path = result.get("path").and_then(Value::as_str).unwrap_or("").to_owned();
            let response = ui.add(
                egui::Label::new(value).truncate().sense(egui::Sense::click())
            );
            if response.double_clicked() && !path.is_empty() {
                open_path_in_default_app(path.clone());
            }
            response.context_menu(|ui| {
                if ui.add_enabled(!path.is_empty(), egui::Button::new(
                    if self.arabic { "فتح الملف" } else { "Open file" }
                )).clicked() {
                    open_path_in_default_app(path.clone());
                    ui.close_menu();
                }
                if ui.add_enabled(!path.is_empty(), egui::Button::new(
                    if self.arabic { "نسخ المسار الكامل" } else { "Copy full path" }
                )).clicked() {
                    ui.ctx().copy_text(path.clone());
                    ui.close_menu();
                }
            });
        } else {
            ui.add(egui::Label::new(value).truncate());
        }
    }

    fn default_row_height(&self) -> f32 {
        self.row_height
    }
}

impl eframe::App for DeepSearchApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.receive_messages();
        if !self.service_checked {
            self.service_checked = true;
            self.check_system_service();
        }
        let dropped_files = ctx.input(|input| input.raw.dropped_files.clone());
        for dropped in dropped_files {
            if let Some(path) = dropped.path {
                let path = path.to_string_lossy().to_string();
                if !self.roots.lines().any(|existing| existing.trim() == path) {
                    if !self.roots.is_empty() && !self.roots.ends_with('\n') {
                        self.roots.push('\n');
                    }
                    self.roots.push_str(&path);
                }
            }
        }
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

        let source_panel = if self.arabic {
            egui::SidePanel::right("sources")
        } else {
            egui::SidePanel::left("sources")
        };
        source_panel.resizable(true).default_width(290.0).show(ctx, |ui| {
            ui.heading(if self.arabic { "الخدمة" } else { "Service" });
            ui.label(if self.arabic { "عنوان الخدمة" } else { "Address" });
            ui.text_edit_singleline(&mut self.service_address);
            if ui.button(if self.arabic { "اتصال / تحديث الحالة" } else { "Connect / Refresh status" }).clicked() {
                self.check_status();
            }
            ui.separator();
            ui.heading(if self.arabic { "مجلدات الفهرسة" } else { "Index locations" });
            ui.label(if self.arabic { "مسار مجلد في كل سطر:" } else { "One folder path per line:" });
            if ui.button(if self.arabic { "استعراض المجلدات…" } else { "Browse folders…" }).clicked() {
                self.pick_folder();
            }
            ui.add(egui::TextEdit::multiline(&mut self.roots).desired_rows(4).desired_width(f32::INFINITY));
            if ui.add_enabled(!self.busy, egui::Button::new(if self.arabic { "ابدأ الفهرسة" } else { "Index locations" })).clicked() {
                self.index_roots();
            }
            ui.separator();
            ui.collapsing(if self.arabic { "إدارة خدمة الخلفية" } else { "Background service" }, |ui| {
                if let Some(active) = self.service_active {
                    ui.label(format!("Runtime: {}", if active { "Running" } else { "Stopped" }));
                }
                if let Some(enabled) = self.service_enabled {
                    let mut startup = enabled;
                    if ui.add_enabled(
                        !self.service_busy,
                        egui::Checkbox::new(&mut startup, if self.arabic { "التشغيل تلقائيًا مع بدء النظام" } else { "Start automatically at boot" }),
                    ).changed() {
                        self.control_system_service(if startup { "enable" } else { "disable" });
                    }
                } else {
                    ui.label(if self.arabic { "حالة بدء التشغيل غير معروفة" } else { "Boot startup state unavailable" });
                }
                ui.horizontal(|ui| {
                    if ui.add_enabled(!self.service_busy, egui::Button::new(if self.arabic { "تشغيل" } else { "Start" })).clicked() {
                        self.control_system_service("start");
                    }
                    if ui.add_enabled(!self.service_busy, egui::Button::new(if self.arabic { "إيقاف" } else { "Stop" })).clicked() {
                        self.control_system_service("stop");
                    }
                    if ui.add_enabled(!self.service_busy, egui::Button::new(if self.arabic { "تحديث" } else { "Refresh" })).clicked() {
                        self.check_system_service();
                    }
                });
                if self.service_busy { ui.spinner(); }
                ui.label(&self.service_detail);
                ui.small(if self.arabic { "مدير خدمات النظام · قد يُطلب تأكيد الصلاحيات" } else { "Platform service manager · elevated authorization may be requested" });
            });
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

        let view_panel = if self.arabic {
            egui::SidePanel::left("view_settings")
        } else {
            egui::SidePanel::right("view_settings")
        };
        view_panel.resizable(true).default_width(270.0).show(ctx, |ui| {
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

fn result_column_text(result: &Value, key: &str) -> String {
    match key {
        "name" | "extension" | "path" => result.get(key).and_then(Value::as_str)
            .unwrap_or("—").to_owned(),
        "modified" => result.get("modified_timestamp_ns").and_then(Value::as_u64)
            .map(format_modified).unwrap_or_else(|| "—".to_owned()),
        "size" => format_size(result.get("size_bytes").and_then(Value::as_u64).unwrap_or(0)),
        _ => "—".to_owned(),
    }
}

fn native_folder_picker() -> Vec<String> {
    use std::process::Command;

    #[cfg(target_os = "linux")]
    {
        match Command::new("zenity")
            .args(["--file-selection", "--directory", "--multiple", "--separator=\n"])
            .output()
        {
            Ok(output) if output.status.success() => {
                return String::from_utf8_lossy(&output.stdout).lines()
                    .map(str::trim).filter(|path| !path.is_empty()).map(str::to_owned).collect();
            }
            Ok(_) => return Vec::new(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Vec::new(),
        }
        match Command::new("kdialog").arg("--getexistingdirectory").output() {
            Ok(output) if output.status.success() => {
                let path = String::from_utf8_lossy(&output.stdout).trim().to_owned();
                if path.is_empty() { Vec::new() } else { vec![path] }
            }
            _ => Vec::new(),
        }
    }

    #[cfg(target_os = "macos")]
    {
        let script = "POSIX path of (choose folder with prompt \"Select a folder to index\")";
        match Command::new("osascript").args(["-e", script]).output() {
            Ok(output) if output.status.success() => {
                let path = String::from_utf8_lossy(&output.stdout).trim().to_owned();
                if path.is_empty() { Vec::new() } else { vec![path] }
            }
            _ => Vec::new(),
        }
    }

    #[cfg(target_os = "windows")]
    {
        let script = "Add-Type -AssemblyName System.Windows.Forms; $d = New-Object System.Windows.Forms.FolderBrowserDialog; if ($d.ShowDialog() -eq 'OK') { [Console]::WriteLine($d.SelectedPath) }";
        match Command::new("powershell.exe").args(["-NoProfile", "-STA", "-Command", script]).output() {
            Ok(output) if output.status.success() => {
                let path = String::from_utf8_lossy(&output.stdout).trim().to_owned();
                if path.is_empty() { Vec::new() } else { vec![path] }
            }
            _ => Vec::new(),
        }
    }
}

#[cfg(target_os = "linux")]
fn service_helper(action: &str, username: &str, privileged: bool) -> Result<String, String> {
    use std::process::Command;
    let output = if privileged {
        Command::new("pkexec")
            .arg("/usr/local/bin/deep-search-service")
            .arg(action)
            .arg(username)
            .output()
    } else {
        Command::new("deep-search-service")
            .arg(action)
            .arg(username)
            .output()
    }.map_err(|error| format!("Could not run service helper: {error}. Is the Linux systemd service installed?"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    let expected_inactive = (action == "is-active" && stdout == "inactive")
        || (action == "is-enabled" && matches!(stdout.as_str(), "disabled" | "static" | "indirect" | "masked"));
    if output.status.success() || expected_inactive {
        Ok(if stdout.is_empty() { if stderr.is_empty() { "OK".to_owned() } else { stderr } } else { stdout })
    } else {
        Err(if stderr.is_empty() { stdout } else { stderr })
    }
}

#[cfg(target_os = "macos")]
fn service_helper(action: &str, _username: &str, privileged: bool) -> Result<String, String> {
    use std::process::Command;
    let helper = "/usr/local/bin/deep-search-service";
    let output = if privileged {
        let script = format!("do shell script \"{helper} {action}\" with administrator privileges");
        Command::new("osascript").args(["-e", &script]).output()
    } else {
        Command::new(helper).arg(action).output()
    }.map_err(|error| format!("Could not run the launchd service helper: {error}. Install the macOS service first."))?;
    command_output_result(output)
}

#[cfg(target_os = "windows")]
fn service_helper(action: &str, _username: &str, privileged: bool) -> Result<String, String> {
    use std::process::Command;
    let program_files = std::env::var_os("ProgramFiles")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Program Files"));
    let helper = program_files.join("Deep-Search").join("deep-search-service.ps1");
    if !helper.exists() {
        return Err(format!("Windows service helper not found at {}. Install the startup task first.", helper.display()));
    }
    let output = if privileged {
        let helper_text = helper.to_string_lossy().replace('\'', "''");
        let argument_line = format!("-NoProfile -ExecutionPolicy Bypass -File \"{helper_text}\" -Action {action}");
        let wrapper = format!(
            "$p = Start-Process -FilePath 'powershell.exe' -Verb RunAs -Wait -PassThru -ArgumentList '{}'; exit $p.ExitCode",
            argument_line.replace('\'', "''")
        );
        Command::new("powershell.exe").args(["-NoProfile", "-Command", &wrapper]).output()
    } else {
        Command::new("powershell.exe")
            .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
            .arg(&helper)
            .args(["-Action", action])
            .output()
    }.map_err(|error| format!("Could not run the Windows service helper: {error}"))?;
    command_output_result(output)
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn service_helper(_action: &str, _username: &str, _privileged: bool) -> Result<String, String> {
    Err("Native service controls are not supported on this platform.".to_owned())
}

fn command_output_result(output: std::process::Output) -> Result<String, String> {
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if output.status.success() {
        Ok(if stdout.is_empty() { if stderr.is_empty() { "OK".to_owned() } else { stderr } } else { stdout })
    } else {
        Err(if stderr.is_empty() { stdout } else { stderr })
    }
}

fn open_path_in_default_app(path: String) {
    thread::spawn(move || {
        use std::process::Command;
        #[cfg(target_os = "windows")]
        let result = Command::new("cmd.exe").args(["/C", "start", ""]).arg(&path).spawn();
        #[cfg(target_os = "macos")]
        let result = Command::new("open").arg(&path).spawn();
        #[cfg(target_os = "linux")]
        let result = Command::new("xdg-open").arg(&path).spawn();
        #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
        let result: Result<std::process::Child, std::io::Error> =
            Err(std::io::Error::other("Opening files is unsupported on this platform"));
        if let Err(error) = result {
            eprintln!("Could not open {}: {error}", path);
        }
    });
}

fn column_label(key: &str, arabic: bool) -> &'static str {
    if arabic {
        match key {
            "name" => "الاسم",
            "extension" => "الامتداد",
            "size" => "الحجم",
            "modified" => "تاريخ التعديل",
            "path" => "المسار",
            _ => "عمود",
        }
    } else {
        match key {
            "name" => "Name",
            "extension" => "Extension",
            "size" => "Size",
            "modified" => "Modified",
            "path" => "Path",
            _ => "Column",
        }
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

fn configure_arabic_font(ctx: &egui::Context) {
    #[cfg(target_os = "windows")]
    let candidates = [
        r"C:\Windows\Fonts\arial.ttf",
        r"C:\Windows\Fonts\tahoma.ttf",
        r"C:\Windows\Fonts\segoeui.ttf",
    ];
    #[cfg(target_os = "macos")]
    let candidates = [
        "/System/Library/Fonts/Supplemental/Arial.ttf",
        "/System/Library/Fonts/Supplemental/Tahoma.ttf",
        "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
    ];
    #[cfg(target_os = "linux")]
    let candidates = [
        "/usr/share/fonts/truetype/noto/NotoSansArabic-Regular.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/TTF/DejaVuSans.ttf",
    ];

    let Some(bytes) = candidates.iter().find_map(|path| std::fs::read(path).ok()) else {
        return;
    };
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "deep_search_arabic_fallback".to_owned(),
        std::sync::Arc::new(egui::FontData::from_owned(bytes)),
    );
    fonts.families.entry(egui::FontFamily::Proportional)
        .or_default().push("deep_search_arabic_fallback".to_owned());
    ctx.set_fonts(fonts);
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
        Box::new(|creation_context| {
            configure_arabic_font(&creation_context.egui_ctx);
            let mut app = DeepSearchApp::default();
            app.check_status();
            Ok(Box::new(app))
        }),
    )
}
