use serde::Deserialize;
use std::collections::HashMap;
use std::hash::Hash;
use std::path::PathBuf;
use walkdir::WalkDir;
use egui::{Label, Sense};

mod settings;

// Communication channel for sending data from background thread to UI thread
#[derive(Debug)]
enum AppMessage {
    DataFetched(Vec<Episode>, Vec<LocalFile>),
    FetchError(String),
}

// Represents a local file found in the directory
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LocalFile {
    pub path: PathBuf,
}

// --- Data Structures for OMDB API Response ---
#[derive(Debug, Deserialize, Default, Clone, PartialEq, Eq, Hash)]
#[serde(rename_all = "PascalCase")]
pub struct Episode {
    pub title: String,
    #[serde(rename = "Episode")]
    pub episode: String,
    #[serde(rename = "imdbID")]
    pub imdb_id: String,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
struct SeasonResponse {
    #[serde(default)]
    pub episodes: Vec<Episode>,
}

// --- Main Application State ---

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct SeriesRenamer {
    pub imdb_link: String,
    pub series_directory: String,
    pub show_process_window: bool,

    #[serde(skip)]
    api_key: String,
    #[serde(skip)]
    episodes: Vec<Episode>,
    #[serde(skip)]
    files: Vec<LocalFile>, // This will now hold only UNASSIGNED files
    #[serde(skip)]
    fetch_status: String,
    #[serde(skip)]
    is_fetching: bool,
    #[serde(skip)]
    receiver: Option<crossbeam_channel::Receiver<AppMessage>>,

    // --- NEW STATE ---
    #[serde(skip)]
    rename_plan: HashMap<Episode, LocalFile>,
    #[serde(skip)]
    show_confirmation_dialog: bool,
}

impl Default for SeriesRenamer {
    fn default() -> Self {
        Self {
            imdb_link: String::new(),
            series_directory: String::new(),
            show_process_window: false,
            api_key: String::new(),
            episodes: Vec::new(),
            files: Vec::new(),
            fetch_status: String::from("Waiting for user input..."),
            is_fetching: false,
            receiver: None,
            rename_plan: HashMap::new(),
            show_confirmation_dialog: false,
        }
    }
}

impl SeriesRenamer {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let mut app: SeriesRenamer = if let Some(storage) = cc.storage {
            eframe::get_value(storage, eframe::APP_KEY).unwrap_or_default()
        } else {
            Default::default()
        };

        match confy::load("series_renamer", None) {
            Ok(settings::AppSettings { api_key }) => {
                app.api_key = api_key;
            }
            Err(e) => {
                app.fetch_status = format!("Error loading config: {}", e);
            }
        };

        app
    }
}

impl eframe::App for SeriesRenamer {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // --- Check for messages from background thread ---
        if self.is_fetching {
            if let Some(rx) = &self.receiver {
                if let Ok(msg) = rx.try_recv() {
                    match msg {
                        AppMessage::DataFetched(episodes, files) => {
                            self.episodes = episodes;
                            self.files = files; // Initially, all files are unassigned
                            self.rename_plan.clear(); // Clear any previous plan
                            self.is_fetching = false;
                            self.fetch_status = format!(
                                "Fetched {} episodes and {} files.",
                                self.episodes.len(),
                                self.files.len()
                            );
                        }
                        AppMessage::FetchError(err_msg) => {
                            self.is_fetching = false;
                            self.fetch_status = err_msg;
                        }
                    }
                }
            }
        }

        // --- Main Window UI ---
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Series Renamer");
            ui.separator();
            ui.horizontal(|ui| {
                ui.label("IMDb Link:");
                ui.text_edit_singleline(&mut self.imdb_link);
            });
            ui.horizontal(|ui| {
                ui.label("Series Directory:");
                ui.label(self.series_directory.as_str());
                if ui.button("Browse...").clicked() {
                    if let Some(path) = rfd::FileDialog::new().pick_folder() {
                        self.series_directory = path.to_string_lossy().to_string();
                    }
                }
            });
            if ui.add_enabled(!self.is_fetching, egui::Button::new("Process")).clicked() {
                if !self.imdb_link.is_empty() && !self.series_directory.is_empty() {
                    self.show_process_window = true;
                    self.is_fetching = true;
                    self.fetch_status = "Fetching data...".to_string();
                    self.episodes.clear();
                    self.files.clear();
                    let (sender, receiver) = crossbeam_channel::unbounded();
                    self.receiver = Some(receiver);
                    let (api_key, imdb_link, series_dir) = (
                        self.api_key.clone(),
                        self.imdb_link.clone(),
                        self.series_directory.clone(),
                    );
                    std::thread::spawn(move || {
                        let files: Vec<LocalFile> = WalkDir::new(series_dir)
                            .into_iter()
                            .filter_map(Result::ok)
                            .filter(|e| e.file_type().is_file())
                            .map(|e| LocalFile {
                                path: e.into_path(),
                            })
                            .collect();
                        let imdb_id = match imdb_link.split('/').find(|s| s.starts_with("tt")) {
                            Some(id) => id.to_string(),
                            None => {
                                let _ = sender.send(AppMessage::FetchError(
                                    "Could not find IMDb ID in link.".to_string(),
                                ));
                                return;
                            }
                        };
                        let request_url = format!(
                            "http://www.omdbapi.com/?i={}&Season=1&apikey={}",
                            imdb_id, api_key
                        );
                        let request = ehttp::Request::get(request_url);
                        ehttp::fetch(request, move |result| match result {
                            Ok(response) if response.ok => {
                                match serde_json::from_slice::<SeasonResponse>(&response.bytes) {
                                    Ok(season) => {
                                        let _ = sender
                                            .send(AppMessage::DataFetched(season.episodes, files));
                                    }
                                    Err(e) => {
                                        let _ = sender.send(AppMessage::FetchError(format!(
                                            "JSON Parse Error: {}",
                                            e
                                        )));
                                    }
                                }
                            }
                            Ok(response) => {
                                let _ = sender.send(AppMessage::FetchError(format!(
                                    "API Error: {} {}",
                                    response.status, response.status_text
                                )));
                            }
                            Err(e) => {
                                let _ = sender
                                    .send(AppMessage::FetchError(format!("Network Error: {}", e)));
                            }
                        });
                    });
                } else {
                    self.fetch_status =
                        "Please provide both an IMDb link and a directory.".to_string();
                }
            }
            ui.label(&self.fetch_status);
        });

        // --- Processing and Confirmation Windows ---
        self.show_dnd_window(ctx);
        self.show_confirmation_window(ctx);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, eframe::APP_KEY, self);
    }
}

/// A temporary container for the result of a drag-and-drop operation.
#[derive(Debug, Clone)]
enum DragDropResult {
    None,
    Assign {
        file: LocalFile,
        episode: Episode,
    },
    Unassign {
        file: LocalFile,
    },
}

// --- Window and UI Logic ---
impl SeriesRenamer {
    fn show_dnd_window(&mut self, ctx: &egui::Context) {
        if !self.show_process_window {
            return;
        }

        let mut is_open = self.show_process_window;
        egui::Window::new("Assign Files to Episodes")
            .id(egui::Id::new("dnd_window"))
            .open(&mut is_open)
            .vscroll(false)
            .resizable(true)
            .default_size([900.0, 600.0])
            .show(ctx, |ui| {
                if self.is_fetching {
                    ui.centered_and_justified(|ui| {
                        ui.spinner();
                        ui.label("Fetching data...");
                    });
                } else if self.episodes.is_empty() {
                    ui.centered_and_justified(|ui| {
                        ui.label(&self.fetch_status);
                    });
                } else {
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        self.dnd_ui(ui);
                    });
                }

                ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
                    ui.add_space(8.0);
                    if !self.rename_plan.is_empty() && ui.button("Confirm Rename Plan").clicked() {
                        self.show_confirmation_dialog = true;
                    }
                    ui.separator();
                });
            });
        self.show_process_window = is_open;
    }

    fn show_confirmation_window(&mut self, ctx: &egui::Context) {
        if !self.show_confirmation_dialog {
            return;
        }
        let mut is_open = self.show_confirmation_dialog;
        egui::Window::new("Confirm Renames")
            .collapsible(false)
            .resizable(false)
            .open(&mut is_open)
            .show(ctx, |ui| {
                ui.label("Are you sure you want to perform the following renames?");
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for (episode, file) in &self.rename_plan {
                        let new_name = format!("{} - {}", episode.episode, episode.title);
                        ui.label(format!(
                            "{} -> {}",
                            file.path.file_name().unwrap().to_str().unwrap(),
                            new_name
                        ));
                    }
                });
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("Confirm").clicked() {
                        println!("Renaming files..."); // Placeholder for Stage 3
                        self.show_confirmation_dialog = false;
                        self.show_process_window = false; // Close DND window after rename
                    }
                    if ui.button("Cancel").clicked() {
                        self.show_confirmation_dialog = false;
                    }
                });
            });
        self.show_confirmation_dialog = is_open;
    }

    /// This is the corrected UI logic for the drag-and-drop view.
    fn dnd_ui(&mut self, ui: &mut egui::Ui) {
        let mut drag_drop_result = DragDropResult::None;
        let main_dnd_id = egui::Id::new("main_dnd_id");

        ui.columns(2, |columns| {
            // --- Left Column: Episodes ---
            egui::Frame::group(columns[0].style()).show(&mut columns[0], |ui| {
                ui.heading("Episodes");
                ui.separator();

                egui::ScrollArea::vertical()
                    .id_source("episodes_scroll_area")
                    .auto_shrink([false; 2])
                    .drag_to_scroll(false)
                    .show(ui, |ui| {
                        for episode in self.episodes.clone() {
                            let (rect, response) = drop_target_slot(ui);

                            // FIX: The line that caused a compile error was removed.
                            // We now use the `response` from the drop_target_slot to check for hover.
                            // This correctly identifies if the mouse is over the designated area.
                            let is_hovered = response.hovered();

                            ui.allocate_ui_at_rect(rect, |child_ui| {
                                egui::Frame::none()
                                    .inner_margin(egui::Margin::same(8))
                                    .show(child_ui, |ui| {
                                        ui.vertical(|ui| {
                                            if let Some(file) = self.rename_plan.get(&episode) {
                                                let item_id = egui::Id::new(&file.path);

                                                let item_response = ui.scope(|ui| {
                                                    ui.add(Label::new(egui::RichText::new(format!("E{}: {}", episode.episode, episode.title)).strong()).selectable(false));
                                                    ui.add(
                                                        Label::new(
                                                            egui::RichText::new(file.path.file_name().unwrap().to_str().unwrap())
                                                                .color(egui::Color32::GREEN)
                                                        )
                                                            .wrap()
                                                            .sense(Sense::hover())
                                                    ).on_hover_text(file.path.to_str().unwrap());
                                                }).response;

                                                let drag_response = ui.interact(item_response.rect, item_id, egui::Sense::drag());
                                                if drag_response.is_pointer_button_down_on() {
                                                    ui.ctx().set_dragged_id(item_id);
                                                    ui.ctx().data_mut(|d| d.insert_temp(main_dnd_id, file.clone()));
                                                }
                                            } else {
                                                ui.add(Label::new(egui::RichText::new(format!("E{}: {}", episode.episode, episode.title)).strong()).selectable(false));
                                                ui.add(Label::new(egui::RichText::new("...drop file here...").weak()).selectable(false));
                                            }
                                        });
                                    });
                            });

                            // We check if the item was dropped in this slot.
                            // This is true if the slot is hovered and the mouse button was released.
                            // The subsequent check for `get_temp` confirms that it was a drag-and-drop operation.
                            if is_hovered && ui.input(|i| i.pointer.any_released()) {
                                if let Some(file) = ui.ctx().data(|d| d.get_temp::<LocalFile>(main_dnd_id)) {
                                    drag_drop_result = DragDropResult::Assign { file, episode: episode.clone() };
                                }
                            }
                        }
                    });
            });

            // --- Right Column: Unassigned Files ---
            let unassign_area_response = egui::Frame::group(columns[1].style()).show(&mut columns[1], |ui| {
                ui.heading("Unassigned Files");
                ui.separator();

                egui::ScrollArea::vertical()
                    .id_source("files_scroll_area")
                    .auto_shrink([false; 2])
                    .drag_to_scroll(false)
                    .show(ui, |ui| {
                        for file in &self.files {
                            let item_id = egui::Id::new(&file.path);
                            if ui.ctx().is_being_dragged(item_id) { continue; }

                            let frame = egui::Frame::group(ui.style()).inner_margin(egui::Margin::same(4));
                            let response = frame.show(ui, |ui| {
                                ui.set_min_height(32.0);
                                ui.centered_and_justified(|ui| {
                                    ui.add(Label::new(file.path.file_name().unwrap().to_str().unwrap()).sense(Sense::hover()))
                                        .on_hover_text(file.path.to_str().unwrap());
                                });
                            }).response;

                            let drag_response = ui.interact(response.rect, item_id, egui::Sense::drag());

                            if drag_response.is_pointer_button_down_on() {
                                ui.ctx().set_dragged_id(item_id);
                                ui.ctx().data_mut(|d| d.insert_temp(main_dnd_id, file.clone()));
                            }
                        }
                    });
            });

            if unassign_area_response.response.hovered() && columns[1].input(|i| i.pointer.any_released()) {
                if let Some(file) = columns[1].ctx().data(|d| d.get_temp::<LocalFile>(main_dnd_id)) {
                    drag_drop_result = DragDropResult::Unassign { file };
                }
            }
        });

        match drag_drop_result {
            DragDropResult::Assign { file, episode } => {
                self.files.retain(|f| f != &file);
                self.rename_plan.retain(|_, v| v != &file);
                self.rename_plan.insert(episode, file);
            }
            DragDropResult::Unassign { file } => {
                self.rename_plan.retain(|_, v| v != &file);
                if !self.files.contains(&file) {
                    self.files.push(file);
                }
            }
            DragDropResult::None => {}
        }

        if ui.input(|i| i.pointer.any_released()) {
            ui.ctx().data_mut(|d| d.remove::<LocalFile>(main_dnd_id));
        }
    }
}

// --- UI Helper Functions ---

fn drop_target_slot(ui: &mut egui::Ui) -> (egui::Rect, egui::Response) {
    let (rect, response) = ui.allocate_at_least(egui::vec2(ui.available_width(), 60.0), egui::Sense::hover());
    let is_hovered = ui.ctx().dragged_id().is_some() && response.hovered();

    let color = if is_hovered {
        egui::Color32::from_rgba_premultiplied(0, 255, 0, 20)
    } else {
        egui::Color32::from_gray(50)
    };
    ui.painter().rect_filled(rect, 4.0, color);
    (rect, response)
}

#[allow(dead_code)]
fn drag_source_slot(ui: &mut egui::Ui) -> (egui::Rect, egui::Response) {
    let (rect, response) = ui.allocate_at_least(egui::vec2(ui.available_width(), 40.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 4.0, egui::Color32::from_gray(60));
    (rect, response)
}
