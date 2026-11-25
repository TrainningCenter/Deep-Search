use deep_search::client::ServiceClient;
use eframe::egui;
use egui_table::{AutoSizeMode, CellInfo, Column as TableColumn, HeaderCellInfo, HeaderRow, Table, TableDelegate, TableState};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};
use tray_icon::{menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem}, Icon as TrayIconImage, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

enum UiMessage {
    Status(Value),
    Indexed(Value),
    SearchProgress { batch: Vec<Value>, loaded: u64, total: u64 },
    SearchFinished { total: u64 },
    Error(String),
    ServiceState { active: Option<bool>, enabled: Option<bool>, detail: String },
    ServiceAction { action: String, success: bool, detail: String },
    ServiceLogoutSetting { enabled: bool, success: bool, detail: String },
    RootSetting { success: bool, detail: String },
    IndexingPause { paused: bool, success: bool, detail: String },
    IndexError(String),
    TrayMenu(String),
    PickedPaths(Vec<String>),
    NasState(Vec<(String, bool)>),
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

#[derive(Clone, Serialize, Deserialize)]
struct SearchCondition {
    field: String,
    operator: String,
    value: String,
    #[serde(default = "default_enabled")]
    enabled: bool,
}

#[derive(Clone, Serialize, Deserialize)]
struct SearchProfile {
    conditions: Vec<SearchCondition>,
    #[serde(default)]
    join_with_or: bool,
}

fn default_search_profile() -> SearchProfile {
    SearchProfile {
        conditions: vec![SearchCondition {
            field: "name".into(),
            operator: "contains".into(),
            value: String::new(),
            enabled: true,
        }],
        join_with_or: false,
    }
}

fn default_frozen_columns() -> usize { 1 }

fn default_enabled() -> bool { true }
fn default_disabled() -> bool { false }

#[derive(Serialize, Deserialize)]
struct UiSettings {
    profiles: BTreeMap<String, ViewProfile>,
    active_profile: String,
    #[serde(default = "default_enabled")]
    tray_enabled: bool,
    #[serde(default = "default_enabled")]
    close_to_tray: bool,
    #[serde(default = "default_disabled")]
    notify_on_completion: bool,
    #[serde(default = "default_enabled")]
    notify_on_errors: bool,
    #[serde(default = "default_disabled")]
    notify_on_service_state: bool,
    #[serde(default)]
    nas_roots: String,
    #[serde(default = "default_disabled")]
    notify_on_nas_state: bool,
    #[serde(default)]
    search_profiles: BTreeMap<String, SearchProfile>,
    #[serde(default)]
    active_search_profile: String,
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            profiles: BTreeMap::new(),
            active_profile: String::new(),
            tray_enabled: true,
            close_to_tray: true,
            notify_on_completion: false,
            notify_on_errors: true,
            notify_on_service_state: false,
            nas_roots: String::new(),
            notify_on_nas_state: false,
            search_profiles: BTreeMap::new(),
            active_search_profile: String::new(),
        }
    }
}

fn default_columns() -> Vec<ColumnSetting> {
    vec![
        ColumnSetting { key: "name".into(), visible: true, width: 260.0 },
        ColumnSetting { key: "extension".into(), visible: true, width: 90.0 },
        ColumnSetting { key: "size".into(), visible: true, width: 110.0 },
        ColumnSetting { key: "modified".into(), visible: false, width: 160.0 },
        ColumnSetting { key: "duration".into(), visible: false, width: 110.0 },
        ColumnSetting { key: "resolution".into(), visible: false, width: 120.0 },
        ColumnSetting { key: "video_codec".into(), visible: false, width: 120.0 },
        ColumnSetting { key: "audio_codec".into(), visible: false, width: 120.0 },
        ColumnSetting { key: "frame_rate".into(), visible: false, width: 110.0 },
        ColumnSetting { key: "container".into(), visible: false, width: 150.0 },
        ColumnSetting { key: "bit_rate".into(), visible: false, width: 110.0 },
        ColumnSetting { key: "sample_rate".into(), visible: false, width: 110.0 },
        ColumnSetting { key: "channels".into(), visible: false, width: 90.0 },
        ColumnSetting { key: "source".into(), visible: true, width: 190.0 },
        ColumnSetting { key: "image_format".into(), visible: false, width: 110.0 },
        ColumnSetting { key: "camera_make".into(), visible: false, width: 150.0 },
        ColumnSetting { key: "camera_model".into(), visible: false, width: 180.0 },
        ColumnSetting { key: "captured_at".into(), visible: false, width: 170.0 },
        ColumnSetting { key: "orientation".into(), visible: false, width: 110.0 },
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
            settings.profiles.insert("Default".into(), default_profile(&["name", "extension", "size", "source", "path"]));
            settings.profiles.insert("Videos".into(), default_profile(&["name", "extension", "duration", "resolution", "video_codec", "audio_codec", "frame_rate", "size", "source", "modified", "path"]));
            settings.profiles.insert("Audio".into(), default_profile(&["name", "extension", "duration", "audio_codec", "bit_rate", "sample_rate", "channels", "size", "path"]));
            settings.profiles.insert("Images".into(), default_profile(&["name", "extension", "resolution", "image_format", "camera_make", "camera_model", "captured_at", "orientation", "size", "path"]));
            settings.profiles.insert("Documents".into(), default_profile(&["name", "extension", "modified", "size", "path"]));
            settings.profiles.insert("Large files".into(), default_profile(&["name", "size", "extension", "path"]));
            settings.profiles.insert("NAS".into(), default_profile(&["name", "size", "source", "modified", "path"]));
            settings.active_profile = "Default".into();
            settings.save();
        }
        // Add newly introduced columns to existing saved profiles without
        // changing which columns the user already chose to show.
        let available_columns = default_columns();
        let mut migrated = false;
        for profile in settings.profiles.values_mut() {
            for candidate in &available_columns {
                if !profile.columns.iter().any(|column| column.key == candidate.key) {
                    profile.columns.push(candidate.clone());
                    migrated = true;
                }
            }
        }
        if !settings.profiles.contains_key("Images") {
            settings.profiles.insert("Images".into(), default_profile(&[
                "name", "extension", "resolution", "image_format", "camera_make",
                "camera_model", "captured_at", "orientation", "size", "path",
            ]));
            migrated = true;
        }
        if migrated { settings.save(); }
        if settings.search_profiles.is_empty() {
            settings.search_profiles.insert("Default".into(), default_search_profile());
            settings.active_search_profile = "Default".into();
            settings.save();
        }
        if !settings.search_profiles.contains_key(&settings.active_search_profile) {
            settings.active_search_profile = settings.search_profiles.keys().next().cloned().unwrap_or_default();
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
    new_search_profile_name: String,
    arabic: bool,
    service_active: Option<bool>,
    service_enabled: Option<bool>,
    indexing_paused: Option<bool>,
    stop_on_logout: Option<bool>,
    logout_detection_available: Option<bool>,
    service_detail: String,
    service_busy: bool,
    service_checked: bool,
    dragged_column: Option<usize>,
    reset_table_layout: bool,
    tray_icon: Option<TrayIcon>,
    tray_checked: bool,
    tray_event_handler_started: bool,
    exit_requested: bool,
    settings_window_open: bool,
    nas_status: BTreeMap<String, bool>,
    index_roots: Vec<(String, bool)>,
    nas_check_in_progress: bool,
    nas_last_check: Option<Instant>,
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
            new_search_profile_name: String::new(),
            arabic: false,
            service_active: None,
            service_enabled: None,
            indexing_paused: None,
            stop_on_logout: None,
            logout_detection_available: None,
            service_detail: "Checking system service status…".to_owned(),
            service_busy: false,
            service_checked: false,
            dragged_column: None,
            reset_table_layout: false,
            tray_icon: None,
            tray_checked: false,
            tray_event_handler_started: false,
            exit_requested: false,
            settings_window_open: false,
            nas_status: BTreeMap::new(),
            index_roots: Vec::new(),
            nas_check_in_progress: false,
            nas_last_check: None,
        }
    }
}

impl DeepSearchApp {
    fn client(&self) -> ServiceClient {
        ServiceClient::new(self.service_address.trim().to_owned())
    }

    fn create_tray_icon(&mut self, ctx: &egui::Context) {
        if self.tray_checked || !self.settings.tray_enabled {
            return;
        }
        self.tray_checked = true;
        let open = MenuItem::with_id("open", "Open Deep-Search", true, None);
        let pause = MenuItem::with_id("pause_resume", "Pause / Resume Indexing", true, None);
        let engine = MenuItem::with_id("toggle_engine", "Start / Stop Engine", true, None);
        let settings = MenuItem::with_id("settings", "Settings", true, None);
        let exit = MenuItem::with_id("exit", "Exit", true, None);
        let separator_one = PredefinedMenuItem::separator();
        let separator_two = PredefinedMenuItem::separator();
        let menu = match Menu::with_items(&[
            &open, &separator_one, &pause, &engine, &separator_two, &settings, &exit,
        ]) {
            Ok(menu) => menu,
            Err(error) => {
                self.message = format!("Could not create tray menu: {error}");
                return;
            }
        };
        let icon = match create_tray_icon_image() {
            Ok(icon) => icon,
            Err(error) => {
                self.message = format!("Could not create tray icon image: {error}");
                return;
            }
        };
        match TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("Deep-Search")
            .with_icon(icon)
            .build()
        {
            Ok(icon) => {
                self.tray_icon = Some(icon);

                // tray-icon's event callbacks are process-global; register them
                // once even if the user disables and later re-enables the tray.
                if !self.tray_event_handler_started {
                // Forward tray callbacks through the egui context so the native
                // event loop wakes even when the main window is hidden.
                    let tray_ctx = ctx.clone();
                    TrayIconEvent::set_event_handler(Some(move |event| {
                    if matches!(event,
                        TrayIconEvent::Click {
                            button: MouseButton::Left,
                            button_state: MouseButtonState::Up,
                            ..
                        } | TrayIconEvent::DoubleClick {
                            button: MouseButton::Left,
                            ..
                        }
                    ) {
                        tray_ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                        tray_ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                    }
                    tray_ctx.request_repaint();
                }));
                    let menu_sender = self.sender.clone();
                    let menu_ctx = ctx.clone();
                    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
                    // egui 0.31 does not run App::update for a hidden root
                    // viewport. Restore it before dispatching the action so
                    // quick controls are handled reliably on every platform.
                    menu_ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    menu_ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                    let _ = menu_sender.send(UiMessage::TrayMenu(event.id.0));
                    menu_ctx.request_repaint();
                }));
                    self.tray_event_handler_started = true;
                }
            }
            Err(error) => {
                self.message = format!("System tray is unavailable: {error}");
            }
        }
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

    fn save_search_profile_as(&mut self) {
        let name = self.new_search_profile_name.trim().to_owned();
        if name.is_empty() { return; }
        let current = self.settings.search_profiles
            .get(&self.settings.active_search_profile)
            .cloned()
            .unwrap_or_else(default_search_profile);
        self.settings.search_profiles.insert(name.clone(), current);
        self.settings.active_search_profile = name;
        self.new_search_profile_name.clear();
        self.persist_settings();
    }

    fn render_search_builder(&mut self, ui: &mut egui::Ui) {
        let names = self.settings.search_profiles.keys().cloned().collect::<Vec<_>>();
        if names.is_empty() {
            self.settings.search_profiles.insert("Default".into(), default_search_profile());
            self.settings.active_search_profile = "Default".into();
        }
        let mut profile_name = self.settings.active_search_profile.clone();
        let mut profile = self.settings.search_profiles.get(&profile_name)
            .cloned().unwrap_or_else(default_search_profile);
        let mut changed = false;
        let mut apply = false;
        let mut save_as = false;
        let mut remove_at = None;
        let mut move_request = None;

        ui.collapsing(display_text(if self.arabic { "منشئ البحث المتقدم" } else { "Advanced Search Builder" }), |ui| {
            ui.label(display_text(if self.arabic {
                "أنشئ شروطًا متعددة، فعّل أو أخفِ كل شرط، ورتّبها واحفظ تخطيطات البحث."
            } else {
                "Build multi-condition searches, enable or hide individual conditions, reorder them, and save search layouts."
            }));
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("search_builder_profiles")
                    .selected_text(display_text(&profile_name))
                    .show_ui(ui, |ui| {
                        for name in &names {
                            if ui.selectable_label(*name == profile_name, display_text(name)).clicked() {
                                profile_name = name.clone();
                                profile = self.settings.search_profiles.get(name)
                                    .cloned().unwrap_or_else(default_search_profile);
                                changed = true;
                            }
                        }
                    });
                if ui.button(display_text(if self.arabic { "حفظ" } else { "Save" })).clicked() {
                    self.settings.search_profiles.insert(profile_name.clone(), profile.clone());
                    changed = true;
                }
            });
            ui.horizontal(|ui| {
                ui.label(display_text(if self.arabic { "ربط الشروط" } else { "Combine conditions" }));
                egui::ComboBox::from_id_salt("search_builder_join")
                    .selected_text(if profile.join_with_or { "OR" } else { "AND" })
                    .show_ui(ui, |ui| {
                        changed |= ui.selectable_value(&mut profile.join_with_or, false, "AND").changed();
                        changed |= ui.selectable_value(&mut profile.join_with_or, true, "OR").changed();
                    });
            });
            let condition_count = profile.conditions.len();
            for index in 0..condition_count {
                let condition = &mut profile.conditions[index];
                ui.horizontal_wrapped(|ui| {
                    changed |= ui.checkbox(&mut condition.enabled, "").changed();
                    egui::ComboBox::from_id_salt(("search_builder_field", index))
                        .selected_text(&condition.field)
                        .show_ui(ui, |ui| {
                            for field in ["name", "path", "source", "content", "extension", "type", "size", "duration"] {
                                if ui.selectable_label(condition.field == field, field).clicked() {
                                    condition.field = field.to_owned();
                                    condition.operator = if field == "size" || field == "duration" { "at least".into() } else if field == "extension" || field == "type" { "is".into() } else { "contains".into() };
                                    changed = true;
                                }
                            }
                        });
                    let operators: &[&str] = if condition.field == "size" || condition.field == "duration" {
                        &["at least", "at most", "is"]
                    } else if condition.field == "extension" || condition.field == "type" {
                        &["is", "is not"]
                    } else {
                        &["contains"]
                    };
                    egui::ComboBox::from_id_salt(("search_builder_operator", index))
                        .selected_text(&condition.operator)
                        .show_ui(ui, |ui| {
                            for operator in operators {
                                if ui.selectable_label(condition.operator == *operator, *operator).clicked() {
                                    condition.operator = (*operator).to_owned();
                                    changed = true;
                                }
                            }
                        });
                    changed |= ui.add(egui::TextEdit::singleline(&mut condition.value)
                        .desired_width(130.0)
                        .hint_text(if condition.field == "size" { "e.g. 2GB" } else if condition.field == "duration" { "e.g. 90s, 2m, 1:30" } else { "Value" })).changed();
                    if condition.field == "duration" {
                        egui::ComboBox::from_id_salt(("duration_presets", index))
                            .selected_text("Presets")
                            .show_ui(ui, |ui| {
                                for preset in ["30s", "1m", "5m", "10m", "15m", "30m", "45m", "1h", "2h", "4h", "8h"] {
                                    if ui.selectable_label(condition.value == preset, preset).clicked() {
                                        condition.value = preset.to_owned();
                                        changed = true;
                                    }
                                }
                            });
                    }
                    if ui.small_button("↑").clicked() && index > 0 { move_request = Some((index, index - 1)); }
                    if ui.small_button("↓").clicked() && index + 1 < condition_count { move_request = Some((index, index + 1)); }
                    if ui.small_button("×").clicked() { remove_at = Some(index); }
                });
            }
            ui.horizontal_wrapped(|ui| {
                if ui.button(display_text(if self.arabic { "إضافة شرط" } else { "Add condition" })).clicked() {
                    profile.conditions.push(SearchCondition {
                        field: "name".into(), operator: "contains".into(), value: String::new(), enabled: true,
                    });
                    changed = true;
                }
                if ui.button(display_text(if self.arabic { "تطبيق البحث" } else { "Apply search" })).clicked() {
                    apply = true;
                }
                ui.add(egui::TextEdit::singleline(&mut self.new_search_profile_name)
                    .desired_width(120.0)
                    .hint_text(display_text(if self.arabic { "اسم التخطيط" } else { "Layout name" })));
                if ui.button(display_text(if self.arabic { "حفظ باسم" } else { "Save as" })).clicked() {
                    self.settings.search_profiles.insert(profile_name.clone(), profile.clone());
                    save_as = true;
                }
            });
            if let Some(query) = search_profile_query(&profile) {
                ui.label(display_text(format!("Query: {query}")));
            } else {
                ui.label(display_text(if self.arabic { "أضف شرطًا مفعّلًا بقيمة." } else { "Add an enabled condition with a value to preview the query." }));
            }
        });

        if let Some((from, to)) = move_request {
            let item = profile.conditions.remove(from);
            profile.conditions.insert(to, item);
            changed = true;
        }
        if let Some(index) = remove_at {
            if index < profile.conditions.len() {
                profile.conditions.remove(index);
                changed = true;
            }
        }
        self.settings.search_profiles.insert(profile_name.clone(), profile.clone());
        self.settings.active_search_profile = profile_name;
        if save_as {
            self.save_search_profile_as();
        } else if changed {
            self.persist_settings();
        }
        if apply {
            if let Some(query) = search_profile_query(&profile) {
                self.search_query = query;
                self.message = "Advanced search query applied. Press Search all results to run it.".to_owned();
            } else {
                self.message = "Add at least one enabled search condition with a value.".to_owned();
            }
        }
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
            display_text("جارٍ فتح نافذة اختيار المجلد…")
        } else {
            "Opening folder picker…".to_owned()
        };
        thread::spawn(move || {
            let paths = native_folder_picker();
            let _ = sender.send(UiMessage::PickedPaths(paths));
        });
    }

    fn set_root_enabled(&mut self, path: String, enabled: bool) {
        let client = self.client();
        let sender = self.sender.clone();
        self.service_busy = true;
        thread::spawn(move || {
            let result = client.set_root_enabled(&path, enabled)
                .and_then(|_| client.status());
            let message = match result {
                Ok(status) => UiMessage::Status(status),
                Err(error) => UiMessage::RootSetting { success: false, detail: error.to_string() },
            };
            let _ = sender.send(message);
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

    fn set_stop_on_logout(&mut self, enabled: bool) {
        let client = self.client();
        let sender = self.sender.clone();
        self.service_busy = true;
        thread::spawn(move || {
            let result = client.set_stop_on_logout(enabled);
            let message = match result {
                Ok(value) => UiMessage::ServiceLogoutSetting {
                    enabled: value.get("stop_on_logout").and_then(Value::as_bool).unwrap_or(enabled),
                    success: true,
                    detail: "Logout preference saved.".to_owned(),
                },
                Err(error) => UiMessage::ServiceLogoutSetting {
                    enabled,
                    success: false,
                    detail: error.to_string(),
                },
            };
            let _ = sender.send(message);
        });
    }

    fn set_indexing_paused(&mut self, paused: bool) {
        let client = self.client();
        let sender = self.sender.clone();
        self.service_busy = true;
        thread::spawn(move || {
            let result = if paused { client.pause_indexing() } else { client.resume_indexing() };
            let message = match result {
                Ok(value) => UiMessage::IndexingPause {
                    paused: value.get("indexing_paused").and_then(Value::as_bool).unwrap_or(paused),
                    success: true,
                    detail: if paused { "Indexing paused.".to_owned() } else { "Indexing resumed.".to_owned() },
                },
                Err(error) => UiMessage::IndexingPause {
                    paused,
                    success: false,
                    detail: error.to_string(),
                },
            };
            let _ = sender.send(message);
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
                .unwrap_or_else(|error| UiMessage::IndexError(error.to_string()));
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
            let progress_sender = sender.clone();
            let result = client.search_with_batches(&query, &sort, &order, move |batch, loaded, total| {
                let _ = progress_sender.send(UiMessage::SearchProgress { batch, loaded, total });
            });
            let message = match result {
                Ok(total) => UiMessage::SearchFinished { total },
                Err(error) => UiMessage::Error(error.to_string()),
            };
            let _ = sender.send(message);
        });
    }

    fn receive_messages(&mut self, ctx: &egui::Context) {
        while let Ok(message) = self.receiver.try_recv() {
            match message {
                UiMessage::Status(status) => {
                    self.busy = false;
                    self.indexed_files = status.get("indexed_files").and_then(Value::as_u64);
                    self.index_roots = status.get("index_roots").and_then(Value::as_array)
                        .map(|roots| roots.iter().filter_map(|root| {
                            Some((root.get("path")?.as_str()?.to_owned(), root.get("enabled")?.as_bool()?))
                        }).collect())
                        .unwrap_or_default();
                    self.service_busy = false;
                    self.stop_on_logout = status.get("stop_on_logout").and_then(Value::as_bool);
                    self.indexing_paused = status.get("indexing_paused").and_then(Value::as_bool);
                    self.logout_detection_available = status.get("logout_detection_available").and_then(Value::as_bool);
                    self.message = format!(
                        "Service connected · version {}",
                        status.get("version").and_then(Value::as_str).unwrap_or("unknown")
                    );
                }
                UiMessage::Indexed(report) => {
                    self.busy = false;
                    let saved = report.get("files_saved").and_then(Value::as_u64).unwrap_or(0);
                    let discovered = report.get("files_discovered").and_then(Value::as_u64).unwrap_or(0);
                    let errors = report.get("errors").and_then(Value::as_u64).unwrap_or(0);
                    self.message = format!(
                        "Indexing finished · {saved} saved · {discovered} discovered · {errors} errors"
                    );
                    if errors > 0 && self.settings.notify_on_errors {
                        show_notification(
                            "Indexing finished with errors",
                            &format!("{errors} error(s) while indexing {discovered} file(s)."),
                        );
                    } else if errors == 0 && self.settings.notify_on_completion {
                        show_notification(
                            "Indexing completed",
                            &format!("{saved} file(s) indexed successfully."),
                        );
                    }
                    self.check_status();
                }
                UiMessage::SearchProgress { batch, loaded, total } => {
                    self.total = total;
                    self.results.extend(batch);
                    self.message = format!("Retrieving results · {loaded} of {total} loaded");
                }
                UiMessage::SearchFinished { total } => {
                    self.busy = false;
                    self.total = total;
                    self.message = format!("Complete · {} of {} results loaded", self.results.len(), self.total);
                }
                UiMessage::IndexError(error) => {
                    self.busy = false;
                    self.message = format!("Indexing failed: {error}");
                    if self.settings.notify_on_errors {
                        show_notification("Indexing failed", &error);
                    }
                }
                UiMessage::Error(error) => {
                    self.busy = false;
                    self.message = format!("Error: {error}");
                }
                UiMessage::ServiceState { active, enabled, detail } => {
                    let state_changed = self.service_active.is_some()
                        && active.is_some()
                        && self.service_active != active;
                    self.service_active = active;
                    self.service_enabled = enabled;
                    self.service_detail = detail;
                    self.service_busy = false;
                    if state_changed && self.settings.notify_on_service_state {
                        let state = if active == Some(true) { "started" } else { "stopped" };
                        show_notification("Deep-Search background service", &format!("Service {state}."));
                    }
                }
                UiMessage::ServiceAction { action, success, detail } => {
                    self.service_detail = if success {
                        format!("{action} completed: {detail}")
                    } else {
                        format!("{action} failed: {detail}")
                    };
                    self.service_busy = false;
                    if success {
                        self.check_system_service();
                    } else if self.settings.notify_on_errors {
                        show_notification("Deep-Search service error", &detail);
                    }
                }
                UiMessage::ServiceLogoutSetting { enabled, success, detail } => {
                    self.service_busy = false;
                    if success {
                        self.stop_on_logout = Some(enabled);
                        self.service_detail = detail;
                    } else {
                        self.service_detail = format!("Could not update logout preference: {detail}");
                    }
                }
                UiMessage::RootSetting { success, detail } => {
                    self.service_busy = false;
                    self.service_detail = if success { detail } else { format!("Could not update search source: {detail}") };
                }
                UiMessage::IndexingPause { paused, success, detail } => {
                    self.service_busy = false;
                    if success {
                        self.indexing_paused = Some(paused);
                        self.service_detail = detail;
                    } else {
                        self.service_detail = format!("Could not change indexing state: {detail}");
                    }
                }
                UiMessage::TrayMenu(action) => {
                    match action.as_str() {
                        "open" => {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                        }
                        "pause_resume" => {
                            if let Some(paused) = self.indexing_paused {
                                self.set_indexing_paused(!paused);
                            } else {
                                self.check_status();
                            }
                        }
                        "toggle_engine" => {
                            let action = if self.service_active == Some(true) { "stop" } else { "start" };
                            self.control_system_service(action);
                        }
                        "settings" => {
                            self.settings_window_open = true;
                            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                        }
                        "exit" => self.exit_requested = true,
                        _ => {}
                    }
                }
                UiMessage::NasState(states) => {
                    self.nas_check_in_progress = false;
                    for (path, available) in states {
                        if let Some(previous) = self.nas_status.insert(path.clone(), available) {
                            if previous != available && self.settings.notify_on_nas_state {
                                let (summary, body) = if available {
                                    ("NAS location reconnected", format!("The network location is available again: {path}"))
                                } else {
                                    ("NAS location disconnected", format!("The network location is unavailable: {path}"))
                                };
                                show_notification(summary, &body);
                            }
                        }
                    }
                }
                UiMessage::PickedPaths(paths) => {
                    self.busy = false;
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
        ui.heading(display_text(if self.arabic { "إعدادات الأعمدة" } else { "Columns & layout" }));
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
            let row = ui.with_layout(horizontal_layout(arabic), |ui| {
                let handle = ui.add(egui::Label::new("⠿").sense(egui::Sense::drag()));
                if handle.drag_started() { drag_started = Some(index); }
                let column = &mut columns[index];
                let label = column_label(&column.key, arabic);
                changed |= ui.checkbox(&mut column.visible, display_text(label)).changed();
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
        if ui.button(display_text(if arabic { "إضافة الأعمدة المتاحة" } else { "Add available columns" })).clicked() {
            for candidate in default_columns() {
                if !columns.iter().any(|existing| existing.key == candidate.key) {
                    columns.push(candidate);
                    changed = true;
                }
            }
        }
        self.dragged_column = next_dragged_column;
        let row_height = &mut self.active_profile_mut().row_height;
        ui.with_layout(horizontal_layout(arabic), |ui| {
            ui.label(display_text(if arabic { "ارتفاع الصف" } else { "Row height" }));
            changed |= ui.add(egui::Slider::new(row_height, 18.0..=60.0).suffix(" px")).changed();
        });
        let visible_count = self.active_profile().columns.iter().filter(|column| column.visible).count();
        let max_frozen = visible_count.min(4);
        let frozen_columns = &mut self.active_profile_mut().frozen_columns;
        ui.with_layout(horizontal_layout(arabic), |ui| {
            ui.label(display_text(if arabic { "تثبيت الأعمدة الأولى" } else { "Freeze leading columns" }));
            changed |= ui.add(egui::Slider::new(frozen_columns, 0..=max_frozen)).changed();
        });
        if *frozen_columns > max_frozen {
            *frozen_columns = max_frozen;
            changed = true;
        }
        if ui.button(display_text(if self.arabic { "ملاءمة تلقائية للأعمدة" } else { "Auto-fit columns" })).clicked() {
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
        ui.label(display_text(format!("{} result(s)", self.total)));
        let mut columns = self.active_profile().columns.iter()
            .filter(|column| column.visible).cloned().collect::<Vec<_>>();
        if self.arabic { columns.reverse(); }
        if columns.is_empty() {
            ui.centered_and_justified(|ui| ui.label(display_text("Show at least one column in Columns & layout.")));
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
        let layout = if self.arabic {
            egui::Layout::right_to_left(egui::Align::Center)
        } else {
            egui::Layout::left_to_right(egui::Align::Center)
        };
        ui.with_layout(layout, |ui| {
            if let Some(column) = self.columns.get(cell.col_range.start) {
                ui.strong(display_text(column_label(&column.key, self.arabic)));
            }
        });
    }

    fn cell_ui(&mut self, ui: &mut egui::Ui, cell: &CellInfo) {
        let layout = if self.arabic {
            egui::Layout::right_to_left(egui::Align::Center)
        } else {
            egui::Layout::left_to_right(egui::Align::Center)
        };
        ui.with_layout(layout, |ui| {
            let Some(column) = self.columns.get(cell.col_nr) else { return; };
            let Some(result) = self.results.get(cell.row_nr as usize) else { return; };
            if cell.row_nr % 2 == 1 {
                ui.painter().rect_filled(ui.max_rect(), 0.0, ui.visuals().faint_bg_color);
            }
            let value = result_column_text(result, &column.key);
            if column.key == "name" {
                let path = result.get("path").and_then(Value::as_str).unwrap_or("").to_owned();
                let response = ui.add(
                    egui::Label::new(display_text(value)).truncate().sense(egui::Sense::click())
                );
                if response.double_clicked() && !path.is_empty() {
                    open_path_in_default_app(path.clone());
                }
                response.context_menu(|ui| {
                    if ui.add_enabled(!path.is_empty(), egui::Button::new(display_text(
                        if self.arabic { "فتح الملف" } else { "Open file" }
                    ))).clicked() {
                        open_path_in_default_app(path.clone());
                        ui.close_menu();
                    }
                    if ui.add_enabled(!path.is_empty(), egui::Button::new(display_text(
                        if self.arabic { "نسخ المسار الكامل" } else { "Copy full path" }
                    ))).clicked() {
                        ui.ctx().copy_text(path.clone());
                        ui.close_menu();
                    }
                });
            } else {
                ui.add(egui::Label::new(display_text(value)).truncate());
            }
        });
    }

    fn default_row_height(&self) -> f32 {
        self.row_height
    }
}

impl eframe::App for DeepSearchApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.receive_messages(ctx);
        self.create_tray_icon(ctx);
        let close_requested = ctx.input(|input| input.viewport().close_requested());
        if close_requested && !self.exit_requested {
            if self.settings.close_to_tray && self.tray_icon.is_some() {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            }
        }
        if self.exit_requested {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if !self.service_checked {
            self.service_checked = true;
            self.check_system_service();
        }
        let nas_roots = self.settings.nas_roots.lines()
            .map(str::trim).filter(|path| !path.is_empty())
            .map(str::to_owned).collect::<Vec<_>>();
        let nas_poll_due = self.nas_last_check
            .map(|last| last.elapsed() >= Duration::from_secs(15))
            .unwrap_or(true);
        if !nas_roots.is_empty() && nas_poll_due && !self.nas_check_in_progress {
            self.nas_check_in_progress = true;
            self.nas_last_check = Some(Instant::now());
            let sender = self.sender.clone();
            thread::spawn(move || {
                let states = nas_roots.into_iter()
                    .map(|path| {
                        let available = nas_path_available(&path);
                        (path, available)
                    })
                    .collect();
                let _ = sender.send(UiMessage::NasState(states));
            });
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
            ui.with_layout(horizontal_layout(self.arabic), |ui| {
                ui.heading(display_text("Deep-Search"));
                ui.separator();
                ui.label(display_text("Desktop Alpha"));
                ui.separator();
                let profiles = self.settings.profiles.keys().cloned().collect::<Vec<_>>();
                let active = self.settings.active_profile.clone();
                egui::ComboBox::from_id_salt("view_profiles")
                    .selected_text(display_text(&active))
                    .show_ui(ui, |ui| {
                        for profile in profiles {
                            if ui.selectable_label(profile == active, &profile).clicked() {
                                self.activate_profile(profile);
                            }
                        }
                    });
                if ui.button("Save profile").clicked() { self.persist_settings(); }
                ui.add(egui::TextEdit::singleline(&mut self.new_profile_name).hint_text(display_text("New profile name")).desired_width(130.0));
                if ui.button("Save as").clicked() { self.save_profile_as(); }
                if ui.button(display_text(if self.arabic { "English" } else { "العربية" })).clicked() {
                    self.arabic = !self.arabic;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let Some(count) = self.indexed_files { ui.label(display_text(format!("{count} indexed files"))); }
                });
            });
        });

        let source_panel = if self.arabic {
            egui::SidePanel::right("sources")
        } else {
            egui::SidePanel::left("sources")
        };
        source_panel.resizable(true).default_width(290.0).show(ctx, |ui| {
            ui.with_layout(
                if self.arabic { egui::Layout::top_down(egui::Align::Max) }
                else { egui::Layout::top_down(egui::Align::Min) },
                |ui| {
            ui.heading(display_text(if self.arabic { "الخدمة" } else { "Service" }));
            ui.label(display_text(if self.arabic { "عنوان الخدمة" } else { "Address" }));
            ui.with_layout(horizontal_layout(self.arabic), |ui| { ui.text_edit_singleline(&mut self.service_address); });
            if ui.button(display_text(if self.arabic { "اتصال / تحديث الحالة" } else { "Connect / Refresh status" })).clicked() {
                self.check_status();
            }
            ui.separator();
            ui.heading(display_text(if self.arabic { "مجلدات الفهرسة" } else { "Index locations" }));
            ui.label(display_text(if self.arabic { "مسار مجلد في كل سطر:" } else { "One folder path per line:" }));
            if ui.button(display_text(if self.arabic { "استعراض المجلدات…" } else { "Browse folders…" })).clicked() {
                self.pick_folder();
            }
            ui.with_layout(horizontal_layout(self.arabic), |ui| {
                ui.add(egui::TextEdit::multiline(&mut self.roots).desired_rows(4).desired_width(f32::INFINITY));
            });
            if ui.add_enabled(!self.busy, egui::Button::new(display_text(if self.arabic { "ابدأ الفهرسة" } else { "Index locations" }))).clicked() {
                self.index_roots();
            }
            ui.separator();
            ui.collapsing(display_text(if self.arabic { "مصادر البحث" } else { "Search sources" }), |ui| {
                if self.index_roots.is_empty() {
                    ui.label(display_text(if self.arabic { "لا توجد مصادر مفهرسة بعد." } else { "No indexed sources yet." }));
                }
                for (path, enabled) in self.index_roots.clone() {
                    let mut included = enabled;
                    if ui.add_enabled(!self.service_busy, egui::Checkbox::new(&mut included, display_text(&path))).changed() {
                        self.set_root_enabled(path, included);
                    }
                }
                ui.small(display_text(if self.arabic {
                    "إلغاء التحديد يستبعد المصدر من البحث فقط، ولا يوقف فهرسته أو يحذف بياناته."
                } else {
                    "Unchecking a source excludes it from searches only; indexing continues and its data is preserved."
                }));
            });
            ui.separator();
            ui.collapsing(display_text(if self.arabic { "إدارة خدمة الخلفية" } else { "Background service" }), |ui| {
                if let Some(active) = self.service_active {
                    ui.label(display_text(format!("Runtime: {}", if active { "Running" } else { "Stopped" })));
                }
                if let Some(enabled) = self.service_enabled {
                    let mut startup = enabled;
                    if ui.add_enabled(
                        !self.service_busy,
                        egui::Checkbox::new(&mut startup, display_text(if self.arabic { "التشغيل تلقائيًا مع بدء النظام" } else { "Start automatically at boot" })),
                    ).changed() {
                        self.control_system_service(if startup { "enable" } else { "disable" });
                    }
                } else {
                    ui.label(display_text(if self.arabic { "حالة بدء التشغيل غير معروفة" } else { "Boot startup state unavailable" }));
                }
                if self.logout_detection_available == Some(false) {
                    ui.label(display_text(if self.arabic {
                        "تعذّر اكتشاف جلسات المستخدم؛ ستظل الخدمة تعمل بعد تسجيل الخروج"
                    } else {
                        "Session detection unavailable; the service will keep running after logout"
                    }));
                } else if let Some(current) = self.stop_on_logout {
                    let mut stop_on_logout = current;
                    if ui.add_enabled(
                        !self.service_busy && self.logout_detection_available == Some(true),
                        egui::Checkbox::new(
                            &mut stop_on_logout,
                            display_text(if self.arabic { "إيقاف الخدمة عند تسجيل الخروج" } else { "Stop service when user logs out" }),
                        ),
                    ).changed() {
                        self.set_stop_on_logout(stop_on_logout);
                    }
                } else {
                    ui.label(display_text(if self.arabic {
                        "اتصل بالخدمة لضبط سلوك تسجيل الخروج"
                    } else {
                        "Connect to the service to configure logout behavior"
                    }));
                }
                if let Some(paused) = self.indexing_paused {
                    if ui.add_enabled(
                        !self.service_busy,
                        egui::Button::new(display_text(if paused {
                            if self.arabic { "استئناف الفهرسة" } else { "Resume indexing" }
                        } else if self.arabic { "إيقاف الفهرسة مؤقتًا" } else { "Pause indexing" })),
                    ).clicked() {
                        self.set_indexing_paused(!paused);
                    }
                }
                ui.with_layout(horizontal_layout(self.arabic), |ui| {
                    if ui.add_enabled(!self.service_busy, egui::Button::new(display_text(if self.arabic { "تشغيل" } else { "Start" }))).clicked() {
                        self.control_system_service("start");
                    }
                    if ui.add_enabled(!self.service_busy, egui::Button::new(display_text(if self.arabic { "إيقاف" } else { "Stop" }))).clicked() {
                        self.control_system_service("stop");
                    }
                    if ui.add_enabled(!self.service_busy, egui::Button::new(display_text(if self.arabic { "تحديث" } else { "Refresh" }))).clicked() {
                        self.check_system_service();
                    }
                });
                if self.service_busy { ui.spinner(); }
                ui.label(display_text(&self.service_detail));
                ui.small(display_text(if self.arabic { "مدير خدمات النظام · قد يُطلب تأكيد الصلاحيات" } else { "Platform service manager · elevated authorization may be requested" }));
            });
            ui.separator();
            ui.heading(display_text(if self.arabic { "بحث" } else { "Search" }));
            ui.with_layout(horizontal_layout(self.arabic), |ui| { ui.text_edit_singleline(&mut self.search_query); });
            egui::ComboBox::from_label(display_text(if self.arabic { "ترتيب حسب" } else { "Sort by" }))
                .selected_text(&self.sort_field).show_ui(ui, |ui| {
                    for field in ["name", "path", "source", "extension", "size", "modified", "relevance"] {
                        ui.selectable_value(&mut self.sort_field, field.to_owned(), field);
                    }
                });
            ui.checkbox(&mut self.descending, display_text(if self.arabic { "ترتيب تنازلي" } else { "Descending" }));
            if ui.add_enabled(!self.busy, egui::Button::new(display_text(if self.arabic { "بحث في كل النتائج" } else { "Search all results" }))).clicked() {
                self.search();
            }
            self.render_search_builder(ui);
            ui.separator();
            if self.busy { ui.spinner(); }
            ui.label(display_text(&self.message));
                },
            );
        });

        let view_panel = if self.arabic {
            egui::SidePanel::left("view_settings")
        } else {
            egui::SidePanel::right("view_settings")
        };
        view_panel.resizable(true).default_width(270.0).show(ctx, |ui| {
            ui.with_layout(
                if self.arabic { egui::Layout::top_down(egui::Align::Max) }
                else { egui::Layout::top_down(egui::Align::Min) },
                |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        ui.heading(display_text(if self.arabic { "إعدادات التطبيق" } else { "Application settings" }));
                        let tray_before = self.settings.tray_enabled;
                        let tray_changed = ui.checkbox(&mut self.settings.tray_enabled, display_text(if self.arabic { "إظهار أيقونة شريط النظام" } else { "Show system tray icon" })).changed();
                        let close_changed = ui.checkbox(&mut self.settings.close_to_tray, display_text(if self.arabic { "إخفاء النافذة عند الإغلاق" } else { "Close window to tray" })).changed();
                        let notify_complete_changed = ui.checkbox(&mut self.settings.notify_on_completion, display_text(if self.arabic { "إشعار عند اكتمال الفهرسة" } else { "Notify when indexing completes" })).changed();
                        let notify_errors_changed = ui.checkbox(&mut self.settings.notify_on_errors, display_text(if self.arabic { "إشعار عند حدوث أخطاء" } else { "Notify when errors occur" })).changed();
                        let notify_service_changed = ui.checkbox(&mut self.settings.notify_on_service_state, display_text(if self.arabic { "إشعار عند تشغيل أو إيقاف الخدمة" } else { "Notify when service starts or stops" })).changed();
                        ui.separator();
                        ui.heading(display_text(if self.arabic { "مراقبة مواقع NAS" } else { "NAS location monitoring" }));
                        ui.label(display_text(if self.arabic {
                            "أدخل مسار كل مشاركة شبكة في سطر منفصل. تتم مراجعة الاتصال كل 15 ثانية."
                        } else {
                            "Enter each mounted network share on a separate line. Availability is checked every 15 seconds."
                        }));
                        let nas_roots_changed = ui.add(
                            egui::TextEdit::multiline(&mut self.settings.nas_roots)
                                .desired_rows(3)
                                .desired_width(f32::INFINITY)
                                .hint_text(display_text("/mnt/nas\\n\\\\server\\share")),
                        ).changed();
                        let notify_nas_changed = ui.checkbox(
                            &mut self.settings.notify_on_nas_state,
                            display_text(if self.arabic { "إشعار عند انقطاع NAS أو عودة الاتصال" } else { "Notify when NAS disconnects or reconnects" }),
                        ).changed();
                        if tray_changed && tray_before != self.settings.tray_enabled {
                            if self.settings.tray_enabled {
                                self.tray_checked = false;
                            } else {
                                self.tray_icon = None;
                                self.tray_checked = true;
                            }
                        }
                        if tray_changed || close_changed || notify_complete_changed || notify_errors_changed || notify_service_changed || nas_roots_changed || notify_nas_changed {
                            self.persist_settings();
                        }
                        ui.separator();
                        self.render_column_controls(ui);
                    });
                },
            );
        });

        if self.settings_window_open {
            egui::Window::new(display_text(if self.arabic { "الإعدادات" } else { "Settings" }))
                .open(&mut self.settings_window_open)
                .show(ctx, |ui| {
                    ui.label(display_text(if self.arabic { "يمكنك ضبط سلوك شريط النظام من لوحة إعدادات التطبيق." } else { "Configure tray visibility and close-to-tray behavior in the application settings panel." }));
                });
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.with_layout(
                if self.arabic { egui::Layout::top_down(egui::Align::Max) }
                else { egui::Layout::top_down(egui::Align::Min) },
                |ui| {
            ui.heading(display_text(if self.arabic { "نتائج البحث" } else { "Search results" }));
            ui.label(display_text(if self.arabic {
                "جدول واحد مستمر · يتم جلب جميع النتائج تلقائيًا · يمكن تغيير عرض الأعمدة"
            } else {
                "One continuous table · all matches are fetched automatically · resize columns by dragging"
            }));
            ui.separator();
            egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| self.render_results(ui));
                },
            );
        });

        if self.busy || self.tray_icon.is_some() || !self.settings.nas_roots.trim().is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(150));
        }
    }
}

fn search_profile_query(profile: &SearchProfile) -> Option<String> {
    let conditions = profile.conditions.iter()
        .filter(|condition| condition.enabled && !condition.value.trim().is_empty())
        .map(|condition| {
            let value = condition.value.trim();
            let safe_value = value.replace('"', "");
            match (condition.field.as_str(), condition.operator.as_str()) {
                ("name", _) => format!("name:\"{safe_value}\""),
                ("path", _) => format!("path:\"{safe_value}\""),
                ("source", _) => format!("source:\"{safe_value}\""),
                ("content", _) => format!("content:\"{safe_value}\""),
                ("extension", "is not") => format!("NOT ext:{}", safe_value.trim_start_matches('.')),
                ("extension", _) => format!("ext:{}", safe_value.trim_start_matches('.')),
                ("type", "is not") => format!("NOT type:{safe_value}"),
                ("type", _) => format!("type:{safe_value}"),
                ("size", "at most") => format!("size<={safe_value}"),
                ("size", "is") => format!("size>={safe_value} AND size<={safe_value}"),
                ("size", _) => format!("size>={safe_value}"),
                ("duration", "at most") => format!("duration<={safe_value}"),
                ("duration", "is") => format!("duration>={safe_value} AND duration<={safe_value}"),
                ("duration", _) => format!("duration>={safe_value}"),
                _ => format!("name:\"{safe_value}\""),
            }
        })
        .collect::<Vec<_>>();
    if conditions.is_empty() { return None; }
    let join = if profile.join_with_or { " OR " } else { " AND " };
    Some(conditions.join(join))
}

fn result_column_text(result: &Value, key: &str) -> String {
    let media = result.get("media_metadata");
    match key {
        "name" | "extension" | "path" | "source" => result.get(key).and_then(Value::as_str)
            .unwrap_or("—").to_owned(),
        "modified" => result.get("modified_timestamp_ns").and_then(Value::as_u64)
            .map(format_modified).unwrap_or_else(|| "—".to_owned()),
        "size" => format_size(result.get("size_bytes").and_then(Value::as_u64).unwrap_or(0)),
        "duration" => media.and_then(|value| value.get("duration_ms")).and_then(Value::as_u64)
            .map(format_duration).unwrap_or_else(|| "—".to_owned()),
        "resolution" => match (
            media.and_then(|value| value.get("width")).and_then(Value::as_u64),
            media.and_then(|value| value.get("height")).and_then(Value::as_u64),
        ) {
            (Some(width), Some(height)) => format!("{width} × {height}"),
            _ => "—".to_owned(),
        },
        "video_codec" | "audio_codec" | "container" | "image_format" | "camera_make" | "camera_model" | "captured_at" | "orientation" => media
            .and_then(|value| value.get(key)).and_then(Value::as_str)
            .unwrap_or("—").to_owned(),
        "frame_rate" => media.and_then(|value| value.get("frame_rate_milli")).and_then(Value::as_u64)
            .map(|rate| format!("{:.2} fps", rate as f64 / 1000.0))
            .unwrap_or_else(|| "—".to_owned()),
        "bit_rate" => media.and_then(|value| value.get("bit_rate")).and_then(Value::as_u64)
            .map(format_bit_rate).unwrap_or_else(|| "—".to_owned()),
        "sample_rate" => media.and_then(|value| value.get("sample_rate")).and_then(Value::as_u64)
            .map(|rate| format!("{:.1} kHz", rate as f64 / 1000.0))
            .unwrap_or_else(|| "—".to_owned()),
        "channels" => media.and_then(|value| value.get("channels")).and_then(Value::as_u64)
            .map(|channels| format!("{channels} ch")).unwrap_or_else(|| "—".to_owned()),
        _ => "—".to_owned(),
    }
}

fn format_duration(duration_ms: u64) -> String {
    let total_seconds = duration_ms / 1000;
    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let seconds = total_seconds % 60;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

fn format_bit_rate(bits_per_second: u64) -> String {
    if bits_per_second >= 1_000_000 {
        format!("{:.2} Mbps", bits_per_second as f64 / 1_000_000.0)
    } else if bits_per_second >= 1_000 {
        format!("{:.0} kbps", bits_per_second as f64 / 1_000.0)
    } else {
        format!("{bits_per_second} bps")
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


fn windows_network_drive_matches(path: &str, network_drives: &[String]) -> bool {
    let normalized_path = path.trim().replace('/', "\\").to_ascii_lowercase();
    network_drives.iter().any(|drive| {
        let root = drive.trim().replace('/', "\\").trim_end_matches('\\').to_ascii_lowercase();
        normalized_path == root || normalized_path.starts_with(&format!("{root}\\"))
    })
}

fn nas_path_available(path: &str) -> bool {
    let target = PathBuf::from(path);
    if !target.is_absolute() {
        return false;
    }

    #[cfg(target_os = "linux")]
    {
        let Ok(mounts) = std::fs::read_to_string("/proc/self/mountinfo") else {
            return false;
        };
        let target = std::fs::canonicalize(&target).unwrap_or(target);
        let matching_network_mount = mounts.lines().filter_map(|line| {
            let (left, right) = line.split_once(" - ")?;
            let fields = left.split_whitespace().collect::<Vec<_>>();
            let fs_type = right.split_whitespace().next()?;
            let mount_point = fields.get(4)?;
            if !is_network_filesystem(fs_type) {
                return None;
            }
            let decoded = mount_point
                .replace("\\040", " ")
                .replace("\\011", "\t")
                .replace("\\012", "\n")
                .replace("\\134", "\\");
            let mount_path = PathBuf::from(decoded);
            target.starts_with(&mount_path).then_some(mount_path)
        }).max_by_key(|mount| mount.components().count());
        return matching_network_mount.is_some() && target.is_dir();
    }

    #[cfg(target_os = "macos")]
    {
        let Ok(output) = std::process::Command::new("mount").output() else {
            return false;
        };
        let listing = String::from_utf8_lossy(&output.stdout);
        let matching_network_mount = listing.lines().filter_map(|line| {
            let (source, rest) = line.split_once(" on ")?;
            let (mount_point, options) = rest.split_once(" (")?;
            let fs_type = options.split([',', ')']).next().unwrap_or("");
            let network = matches!(fs_type, "smbfs" | "nfs" | "afpfs" | "webdav");
            let mount_path = PathBuf::from(mount_point);
            (network && target.starts_with(&mount_path)).then_some(mount_path)
        }).max_by_key(|mount| mount.components().count());
        return matching_network_mount.is_some() && target.is_dir();
    }

    #[cfg(target_os = "windows")]
    {
        let value = path.trim_start();
        if value.starts_with("\\\\") {
            return target.is_dir();
        }
        let output = match std::process::Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "[System.IO.DriveInfo]::GetDrives() | Where-Object { $_.DriveType -eq [System.IO.DriveType]::Network } | ForEach-Object { $_.Name }",
            ])
            .output()
        {
            Ok(output) if output.status.success() => output,
            _ => return false,
        };
        let drives = String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        return windows_network_drive_matches(value, &drives) && target.is_dir();
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        false
    }
}

#[cfg(target_os = "linux")]
fn is_network_filesystem(fs_type: &str) -> bool {
    matches!(
        fs_type,
        "nfs" | "nfs4" | "cifs" | "smb3" | "9p" | "ceph" |
        "glusterfs" | "sshfs" | "fuse.sshfs" | "davfs" | "fuse.rclone" | "fuse.s3fs"
    )
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

fn horizontal_layout(arabic: bool) -> egui::Layout {
    if arabic {
        egui::Layout::right_to_left(egui::Align::Center)
    } else {
        egui::Layout::left_to_right(egui::Align::Center)
    }
}

fn column_label(key: &str, arabic: bool) -> &'static str {
    if arabic {
        match key {
            "name" => "الاسم",
            "extension" => "الامتداد",
            "size" => "الحجم",
            "modified" => "تاريخ التعديل",
            "duration" => "المدة",
            "resolution" => "الدقة",
            "video_codec" => "ترميز الفيديو",
            "audio_codec" => "ترميز الصوت",
            "frame_rate" => "معدل الإطارات",
            "container" => "الحاوية",
            "bit_rate" => "معدل البت",
            "sample_rate" => "معدل العينة",
            "channels" => "القنوات",
            "image_format" => "تنسيق الصورة",
            "camera_make" => "الشركة المصنّعة",
            "camera_model" => "طراز الكاميرا",
            "captured_at" => "تاريخ الالتقاط",
            "orientation" => "الاتجاه",
            "source" => "المصدر",
            "path" => "المسار",
            _ => "عمود",
        }
    } else {
        match key {
            "name" => "Name",
            "extension" => "Extension",
            "size" => "Size",
            "modified" => "Modified",
            "duration" => "Duration",
            "resolution" => "Resolution",
            "video_codec" => "Video codec",
            "audio_codec" => "Audio codec",
            "frame_rate" => "Frame rate",
            "container" => "Container",
            "bit_rate" => "Bit rate",
            "sample_rate" => "Sample rate",
            "channels" => "Channels",
            "image_format" => "Image format",
            "camera_make" => "Camera make",
            "camera_model" => "Camera model",
            "captured_at" => "Date taken",
            "orientation" => "Orientation",
            "source" => "Source",
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

/// Convert logical Arabic text into shaped, visually ordered text for egui's
/// left-to-right text renderer. Input values remain untouched for editing,
/// indexing, sorting, and clipboard operations.
fn display_text(text: impl AsRef<str>) -> String {
    let text = text.as_ref();
    if !text.chars().any(arabic::is_arabic_letter) {
        return text.to_owned();
    }

    let mut visual = String::with_capacity(text.len());
    for segment in text.split_inclusive('\n') {
        let (line, newline) = segment.strip_suffix('\n')
            .map(|line| (line, "\n"))
            .unwrap_or((segment, ""));
        let shaped = arabic::reshape(line, arabic::Direction::Logical);
        let bidi = unicode_bidi::BidiInfo::new(&shaped, None);
        if let Some(paragraph) = bidi.paragraphs.first() {
            visual.push_str(&bidi.reorder_line(paragraph, paragraph.range.clone()));
        } else {
            visual.push_str(&shaped);
        }
        visual.push_str(newline);
    }
    visual
}

fn show_notification(summary: &str, body: &str) {
    let summary = summary.to_owned();
    let body = body.to_owned();
    thread::spawn(move || {
        if let Err(error) = notify_rust::Notification::new()
            .summary(&summary)
            .body(&body)
            .show()
        {
            eprintln!("Could not show desktop notification: {error}");
        }
    });
}

fn create_tray_icon_image() -> Result<TrayIconImage, tray_icon::BadIcon> {
    const SIZE: usize = 32;
    let mut rgba = vec![0_u8; SIZE * SIZE * 4];
    for y in 0..SIZE {
        for x in 0..SIZE {
            let dx = x as i32 - 12;
            let dy = y as i32 - 12;
            let distance = dx * dx + dy * dy;
            let lens_ring = (58..=116).contains(&distance);
            let handle = x >= 20 && x <= 27 && y >= 20 && y <= 27
                && (x as i32 - y as i32).abs() <= 2;
            if distance <= 225 {
                let offset = (y * SIZE + x) * 4;
                rgba[offset] = 35;
                rgba[offset + 1] = 100;
                rgba[offset + 2] = 190;
                rgba[offset + 3] = 255;
            }
            if lens_ring || handle {
                let offset = (y * SIZE + x) * 4;
                rgba[offset] = 245;
                rgba[offset + 1] = 248;
                rgba[offset + 2] = 255;
                rgba[offset + 3] = 255;
            }
        }
    }
    TrayIconImage::from_rgba(rgba, SIZE as u32, SIZE as u32)
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

#[cfg(test)]
mod rtl_text_tests {
    use super::{display_text, search_profile_query, SearchCondition, SearchProfile, UiSettings};

    #[test]
    fn advanced_search_builder_generates_boolean_queries() {
        let profile = SearchProfile {
            conditions: vec![
                SearchCondition { field: "name".into(), operator: "contains".into(), value: "holiday video".into(), enabled: true },
                SearchCondition { field: "size".into(), operator: "at least".into(), value: "1GB".into(), enabled: true },
            ],
            join_with_or: false,
        };
        assert_eq!(
            search_profile_query(&profile).as_deref(),
            Some(r#"name:"holiday video" AND size>=1GB"#)
        );
    }

    #[test]
    fn advanced_search_builder_supports_exact_size_and_ignores_disabled_conditions() {
        let profile = SearchProfile {
            conditions: vec![
                SearchCondition { field: "size".into(), operator: "is".into(), value: "500MB".into(), enabled: true },
                SearchCondition { field: "extension".into(), operator: "is".into(), value: "tmp".into(), enabled: false },
            ],
            join_with_or: false,
        };
        assert_eq!(
            search_profile_query(&profile).as_deref(),
            Some("size>=500MB AND size<=500MB")
        );
    }

    #[test]
    fn advanced_search_builder_generates_exact_duration_ranges() {
        let profile = SearchProfile {
            conditions: vec![SearchCondition {
                field: "duration".into(),
                operator: "is".into(),
                value: "1:30".into(),
                enabled: true,
            }],
            join_with_or: false,
        };
        assert_eq!(
            search_profile_query(&profile).as_deref(),
            Some("duration>=1:30 AND duration<=1:30")
        );
    }

    #[test]
    fn advanced_search_builder_returns_none_without_active_values() {
        let profile = SearchProfile {
            conditions: vec![
                SearchCondition { field: "name".into(), operator: "contains".into(), value: "  ".into(), enabled: true },
                SearchCondition { field: "path".into(), operator: "contains".into(), value: "ignored".into(), enabled: false },
            ],
            join_with_or: true,
        };
        assert!(search_profile_query(&profile).is_none());
    }

    #[test]
    fn arabic_display_text_shapes_letters_and_preserves_latin_runs() {
        let logical = "السلام عليكم report 2026";
        let visual = display_text(logical);
        assert_ne!(visual, logical);
        assert!(visual.contains("report"));
        assert!(visual.contains("2026"));
        assert!(visual.chars().any(|character| ('\u{FE70}'..='\u{FEFF}').contains(&character)));
    }

    #[test]
    fn left_to_right_text_is_unchanged() {
        let text = "Deep-Search report 2026";
        assert_eq!(display_text(text), text);
    }

    #[test]
    fn notification_defaults_follow_opt_in_policy() {
        let settings = UiSettings::default();
        assert!(settings.notify_on_errors);
        assert!(!settings.notify_on_completion);
        assert!(!settings.notify_on_service_state);
        assert!(!settings.notify_on_nas_state);
    }

    #[test]
    fn windows_nas_detection_recognizes_mapped_drives_case_insensitively() {
        let drives = vec!["Z:\\".to_owned(), "Y:\\".to_owned()];
        assert!(super::windows_network_drive_matches("z:\\media\\movie.mkv", &drives));
        assert!(super::windows_network_drive_matches("Y:/backup", &drives));
        assert!(!super::windows_network_drive_matches("C:\\media\\movie.mkv", &drives));
        assert!(!super::windows_network_drive_matches("Z:\\other", &["Z:\\media".to_owned()]));
    }

    #[test]
    fn arabic_display_text_preserves_line_breaks() {
        let text = "مرحبا\nDeep-Search";
        let visual = display_text(text);
        assert!(visual.contains('\n'));
        assert!(visual.ends_with("Deep-Search"));
    }
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
