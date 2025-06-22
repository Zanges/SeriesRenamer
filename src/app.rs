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

// Action to be taken after the confirmation dialog is closed
enum DialogAction {
    Confirm,
    Cancel,
}

// --- Main Application State ---

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct SeriesRenamer {
    pub imdb_link: String,
    pub series_directory: String,
    pub season_number: u32,
    pub show_process_window: bool,

    #[serde(skip)]
    api_key: String,
    #[serde(skip)]
    episodes: Vec<Episode>,
    #[serde(skip)]
    files: Vec<LocalFile>,
    #[serde(skip)]
    fetch_status: String,
    #[serde(skip)]
    is_fetching: bool,
    #[serde(skip)]
    receiver: Option<crossbeam_channel::Receiver<AppMessage>>,

    // The final plan to be confirmed
    #[serde(skip)]
    rename_plan: HashMap<Episode, LocalFile>,
    // Holds the text from the input fields
    #[serde(skip)]
    file_episode_inputs: HashMap<PathBuf, String>,
    #[serde(skip)]
    show_confirmation_dialog: bool,
    // Holds the action to be taken after the confirmation dialog
    #[serde(skip)]
    action_after_confirm: Option<DialogAction>,
}

impl Default for SeriesRenamer {
    fn default() -> Self {
        Self {
            imdb_link: String::new(),
            series_directory: String::new(),
            season_number: 1,
            show_process_window: false,
            api_key: String::new(),
            episodes: Vec::new(),
            files: Vec::new(),
            fetch_status: String::from("Waiting for user input..."),
            is_fetching: false,
            receiver: None,
            rename_plan: HashMap::new(),
            file_episode_inputs: HashMap::new(),
            show_confirmation_dialog: false,
            action_after_confirm: None,
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
                            self.files = files;
                            self.rename_plan.clear();
                            self.file_episode_inputs.clear(); // Clear old inputs
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
                ui.label("Season:");
                ui.add(egui::DragValue::new(&mut self.season_number).range(1..=99));
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
                    let (api_key, imdb_link, series_dir, season_number) = (
                        self.api_key.clone(),
                        self.imdb_link.clone(),
                        self.series_directory.clone(),
                        self.season_number,
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
                            "http://www.omdbapi.com/?i={}&Season={}&apikey={}",
                            imdb_id, season_number, api_key
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

            ui.separator();

            if ui.button("Open Settings").clicked() {
                match confy::get_configuration_file_path("series_renamer", None) {
                    Ok(path) => {
                        if let Err(e) = open::that(&path) {
                            self.fetch_status = format!("Failed to open settings file: {}", e);
                        }
                    }
                    Err(e) => {
                        self.fetch_status = format!("Could not find settings file: {}", e);
                    }
                }
            }

            if ui.button("Get API Key").clicked() {
                let url = "http://www.omdbapi.com/apikey.aspx";
                if let Err(e) = open::that(url) {
                    self.fetch_status = format!("Failed to open URL: {}", e);
                }
            }
        });

        // --- Processing and Confirmation Windows ---
        self.show_assignment_window(ctx);
        self.show_confirmation_window(ctx);

        // --- Handle deferred actions ---
        if let Some(action) = self.action_after_confirm.take() {
            match action {
                DialogAction::Confirm => {
                    let mut rename_results = Vec::new();
                    for (episode, file) in &self.rename_plan {
                        let original_path = &file.path;
                        if let Some(extension) = original_path.extension().and_then(|s| s.to_str())
                        {
                            if let Ok(episode_number) = episode.episode.parse::<u32>() {
                                let sanitized_title = Self::sanitize_title(&episode.title);
                                let new_name = format!(
                                    "S{:02}E{:02} - {}.{}",
                                    self.season_number, episode_number, sanitized_title, extension
                                );

                                if let Some(parent_dir) = original_path.parent() {
                                    let new_path = parent_dir.join(&new_name);
                                    match std::fs::rename(original_path, &new_path) {
                                        Ok(_) => {
                                            rename_results.push(format!(
                                                "Successfully renamed '{}' to '{}'",
                                                original_path.display(),
                                                new_name
                                            ));
                                        }
                                        Err(e) => {
                                            rename_results.push(format!(
                                                "ERROR renaming {}: {}",
                                                original_path.display(),
                                                e
                                            ));
                                        }
                                    }
                                } else {
                                    rename_results.push(format!(
                                        "ERROR: Could not get parent directory for {}",
                                        original_path.display()
                                    ));
                                }
                            } else {
                                rename_results.push(format!(
                                    "ERROR: Could not parse episode number '{}' for {}",
                                    episode.episode,
                                    original_path.display()
                                ));
                            }
                        } else {
                            rename_results.push(format!(
                                "ERROR: Could not get file extension for {}",
                                original_path.display()
                            ));
                        }
                    }

                    self.fetch_status = rename_results.join("\n");
                    self.show_confirmation_dialog = false;
                    self.show_process_window = false;
                    self.rename_plan.clear();
                    self.episodes.clear();
                    self.files.clear();
                    self.file_episode_inputs.clear();
                }
                DialogAction::Cancel => {
                    self.show_confirmation_dialog = false;
                }
            }
        }
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, eframe::APP_KEY, self);
    }
}

// --- Window and UI Logic ---
impl SeriesRenamer {
    fn sanitize_title(title: &str) -> String {
        title.chars().filter(|c| c.is_alphanumeric() || c.is_whitespace()).collect()
    }

    fn show_assignment_window(&mut self, ctx: &egui::Context) {
        if !self.show_process_window {
            return;
        }

        let mut is_open = self.show_process_window;
        egui::Window::new("Assign Files to Episodes")
            .id(egui::Id::new("assignment_window"))
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
                    self.assignment_ui(ui);
                }

                ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
                    ui.add_space(8.0);
                    if !self.files.is_empty() && ui.button("Confirm Rename Plan").clicked() {
                        self.build_rename_plan();
                        if !self.rename_plan.is_empty() {
                            self.show_confirmation_dialog = true;
                        }
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

        egui::Window::new("Confirm Renames")
            .collapsible(false)
            .resizable(false)
            .open(&mut self.show_confirmation_dialog)
            .show(ctx, |ui| {
                ui.label("Are you sure you want to perform the following renames?");
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for (episode, file) in &self.rename_plan {
                        let extension = file.path.extension().and_then(|s| s.to_str()).unwrap_or("");
                        let sanitized_title = Self::sanitize_title(&episode.title);
                        let new_name = if let Ok(episode_number) = episode.episode.parse::<u32>() {
                            format!("S{:02}E{:02} - {}.{}", self.season_number, episode_number, sanitized_title, extension)
                        } else {
                            format!("S{:02}E{} - {}.{}", self.season_number, episode.episode, sanitized_title, extension)
                        };
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
                        self.action_after_confirm = Some(DialogAction::Confirm);
                    }
                    if ui.button("Cancel").clicked() {
                        self.action_after_confirm = Some(DialogAction::Cancel);
                    }
                });
            });
    }

    /// Builds the rename plan from the user's text inputs.
    fn build_rename_plan(&mut self) {
        self.rename_plan.clear();

        // Create a quick lookup map from episode number string to the Episode struct.
        let episode_map: HashMap<String, Episode> = self
            .episodes
            .iter()
            .map(|e| (e.episode.clone(), e.clone()))
            .collect();

        for file in &self.files {
            // Get the user's input for the current file.
            if let Some(episode_num_str) = self.file_episode_inputs.get(&file.path) {
                // If the input is not empty, find the corresponding episode.
                if !episode_num_str.is_empty() {
                    if let Some(episode) = episode_map.get(episode_num_str) {
                        // We found a match, add it to the plan.
                        self.rename_plan.insert(episode.clone(), file.clone());
                    }
                }
            }
        }
    }

    /// This function contains the primary UI logic for manual assignment.
    fn assignment_ui(&mut self, ui: &mut egui::Ui) {
        ui.columns(2, |columns| {
            // --- Left Column: Episodes List (Reference) ---
            let left_ui = &mut columns[0];
            egui::Frame::group(left_ui.style()).show(left_ui, |ui| {
                ui.heading("Episodes");
                ui.separator();

                egui::ScrollArea::vertical()
                    .id_salt("episodes_scroll_area")
                    .show(ui, |ui| {
                        for episode in &self.episodes {
                            ui.label(format!("E{}: {}", episode.episode, episode.title));
                            ui.separator();
                        }
                    });
            });

            // --- Right Column: Files with Input Fields ---
            let right_ui = &mut columns[1];
            egui::Frame::group(right_ui.style()).show(right_ui, |ui| {
                ui.heading("Files");
                ui.separator();

                egui::ScrollArea::vertical()
                    .id_salt("files_scroll_area")
                    .show(ui, |ui| {
                        for file in &self.files {
                            ui.horizontal(|ui| {
                                // Get the mutable string buffer for this file's input field.
                                let buffer = self.file_episode_inputs.entry(file.path.clone()).or_default();

                                // Show the text widget.
                                ui.add(
                                    egui::TextEdit::singleline(buffer)
                                        .hint_text("Ep #")
                                        .desired_width(40.0)
                                );

                                // Show the filename next to the input.
                                ui.label(file.path.file_name().unwrap().to_str().unwrap())
                                    .on_hover_text(file.path.to_str().unwrap());
                            });
                            ui.separator();
                        }
                    });
            });
        });
    }
}