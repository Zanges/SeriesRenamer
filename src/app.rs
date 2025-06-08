use serde::Deserialize;
use std::collections::HashMap;
use std::hash::Hash;
use std::path::PathBuf;
use walkdir::WalkDir;

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

// --- Window and UI Logic ---
impl SeriesRenamer {
    fn show_dnd_window(&mut self, ctx: &egui::Context) {
        if !self.show_process_window {
            return;
        }

        let mut is_open = self.show_process_window;
        egui::Window::new("Assign Files to Episodes")
            .open(&mut is_open)
            .vscroll(true)
            .default_size([900.0, 600.0])
            .show(ctx, |ui| {
                if self.is_fetching {
                    ui.spinner();
                } else if self.episodes.is_empty() {
                    ui.label(&self.fetch_status);
                } else {
                    self.dnd_ui(ui);
                }
                ui.separator();
                if !self.rename_plan.is_empty() && ui.button("Confirm Rename Plan").clicked() {
                    self.show_confirmation_dialog = true;
                }
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
                        ui.label(format!(
                            "{} -> {}",
                            file.path.file_name().unwrap().to_str().unwrap(),
                            episode.title
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

    fn dnd_ui(&mut self, ui: &mut egui::Ui) {
        // Get all the state we need from `ui.ctx()` *before* the conflicting borrow.
        let dragged_id = ui.ctx().dragged_id();
        let is_anything_dragged = dragged_id.is_some();
        let pointer_pos = ui.ctx().pointer_interact_pos();
        let pointer_released = ui.input(|i| i.pointer.primary_released());
        let style = ui.style().clone();

        let mut file_to_unassign: Option<LocalFile> = None;
        let mut dropped_on_episode: Option<(Episode, LocalFile)> = None;

        // Extract the dragged file data here if the pointer is released.
        if pointer_released {
            if let Some(id) = dragged_id {
                if let Some(file) = ui.ctx().data_mut(|d| d.get_temp::<LocalFile>(id)) {
                    file_to_unassign = Some(file.clone());
                }
            }
        }

        ui.columns(2, |columns| {
            // --- Left Column: Episodes (Drop Target) ---
            columns[0].vertical_centered_justified(|ui| {
                ui.heading("Episodes");
            });
            egui::ScrollArea::vertical().show(&mut columns[0], |ui| {
                for episode in self.episodes.clone() {
                    let (rect, response) = drop_target_slot(ui);
                    let is_hovered = is_anything_dragged && response.hovered();

                    if is_hovered {
                        ui.painter().rect_filled(rect, 4.0, egui::Color32::from_rgba_premultiplied(0, 255, 0, 20));
                    }

                    // Correctly allocate a sub-ui for the episode slot.
                    // This uses a deprecated function, but it's the simplest fix for the immediate error.
                    // The modern approach would be `ui.child_ui(rect, *ui.layout())`.
                    ui.allocate_ui_at_rect(rect, |ui| {
                        ui.vertical_centered(|ui| {
                            ui.label(format!("E{}: {}", episode.episode, episode.title));
                            if let Some(file) = self.rename_plan.get(&episode) {
                                // Allow dragging assigned files *out* of the slot
                                let item_id = egui::Id::new(&file.path);
                                // Interact with the full area of the sub-ui.
                                let assigned_response = ui.interact(ui.max_rect(), item_id, egui::Sense::drag());
                                if assigned_response.is_pointer_button_down_on() {
                                    ui.ctx().set_dragged_id(item_id);
                                    ui.ctx().data_mut(|d| d.insert_temp(item_id, file.clone()));
                                }
                                ui.colored_label(
                                    egui::Color32::GREEN,
                                    file.path.file_name().unwrap().to_str().unwrap(),
                                );
                            } else {
                                ui.weak("...drop file here...");
                            }
                        });
                    });


                    // Handle drop
                    if is_anything_dragged && pointer_released && is_hovered {
                        if let Some(id) = dragged_id {
                            if let Some(file) = ui.ctx().data_mut(|d| d.get_temp::<LocalFile>(id)) {
                                dropped_on_episode = Some((episode.clone(), file.clone()));
                            }
                        }
                    }
                }
            });

            // --- Right Column: Files (Drag Source & Drop Target for Un-assigning) ---
            columns[1].vertical_centered_justified(|ui| {
                ui.heading("Unassigned Files");
            });
            let right_column_rect = egui::Frame::group(&style)
                .show(&mut columns[1], |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        self.files.retain_mut(|file| {
                            let item_id = egui::Id::new(&file.path);
                            if ui.ctx().is_being_dragged(item_id) {
                                return true;
                            }

                            let (rect, _response) = drag_source_slot(ui);
                            if ui.interact(rect, item_id, egui::Sense::drag()).is_pointer_button_down_on() {
                                ui.ctx().set_dragged_id(item_id);
                                ui.ctx().data_mut(|d| d.insert_temp(item_id, file.clone()));
                            }

                            ui.put(rect, |ui: &mut egui::Ui| {
                                ui.label(file.path.file_name().unwrap().to_str().unwrap())
                            });

                            true
                        });
                    });
                })
                .response
                .rect;

            // Handle dropping a file on the right column to un-assign it
            let is_hovered = pointer_pos.map_or(false, |p| right_column_rect.contains(p));
            if is_anything_dragged && pointer_released && is_hovered {
                if let Some(file) = &file_to_unassign {
                    if let Some(ep) = self.rename_plan.iter().find_map(|(ep, f)| if f == file { Some(ep.clone()) } else { None }) {
                        self.rename_plan.remove(&ep);
                        if !self.files.contains(file) {
                            self.files.push(file.clone());
                        }
                    }
                }
            }
        });

        // --- Post-UI-layout state mutations ---
        if let Some((episode, file)) = dropped_on_episode {
            if let Some(old_ep) = self.rename_plan.iter().find_map(|(ep, f)| if f == &file { Some(ep.clone()) } else { None }) {
                self.rename_plan.remove(&old_ep);
            }
            if let Some(old_file) = self.rename_plan.insert(episode, file.clone()) {
                if !self.files.contains(&old_file) {
                    self.files.push(old_file);
                }
            }
            self.files.retain(|f| f != &file);
        }

        // Clear drag data on release
        if pointer_released {
            // Use egui::Id::NULL instead of egui::Id::nil()
            ui.ctx().set_dragged_id(egui::Id::NULL);
        }
    }
}

// --- UI Helper Functions ---

// Removed the `is_hovered` parameter as it's better to handle drawing logic outside
fn drop_target_slot(ui: &mut egui::Ui) -> (egui::Rect, egui::Response) {
    let (rect, response) =
        ui.allocate_at_least(egui::vec2(ui.available_width(), 50.0), egui::Sense::hover());

    ui.painter().rect_filled(rect, 4.0, egui::Color32::from_gray(50));

    (rect, response)
}

fn drag_source_slot(ui: &mut egui::Ui) -> (egui::Rect, egui::Response) {
    let (rect, response) =
        ui.allocate_at_least(egui::vec2(ui.available_width(), 40.0), egui::Sense::hover());

    ui.painter()
        .rect_filled(rect, 4.0, egui::Color32::from_gray(60));

    (rect, response)
}
