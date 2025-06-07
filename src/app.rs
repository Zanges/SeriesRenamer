use serde::Deserialize;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

mod settings;

// Communication channel for sending data from background thread to UI thread
#[derive(Debug)] // Added derive for easier debugging
enum AppMessage {
    DataFetched(Vec<Episode>, Vec<LocalFile>),
    FetchError(String),
}

// Represents a local file found in the directory
#[derive(Debug, Clone, PartialEq, Eq, Hash)] // Added derive for drag-and-drop state
pub struct LocalFile {
    pub path: PathBuf,
}

// --- Data Structures for OMDB API Response ---
// We only care about a few fields, so we can ignore the rest with `#[serde(default)]`

#[derive(Debug, Deserialize, Default, Clone, PartialEq, Eq, Hash)] // Added derive for drag-and-drop state
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

    // We use `#[serde(skip)]` to avoid saving this runtime state
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

    // Using Option because the receiver is only present during a fetch operation.
    #[serde(skip)]
    receiver: Option<crossbeam_channel::Receiver<AppMessage>>,
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

        // Load settings from config file
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
                match rx.try_recv() {
                    Ok(AppMessage::DataFetched(episodes, files)) => {
                        self.episodes = episodes;
                        self.files = files;
                        self.is_fetching = false;
                        self.fetch_status = format!("Fetched {} episodes and {} files.", self.episodes.len(), self.files.len());
                    }
                    Ok(AppMessage::FetchError(err_msg)) => {
                        self.is_fetching = false;
                        self.fetch_status = err_msg;
                    }
                    Err(crossbeam_channel::TryRecvError::Empty) => {
                        // Still waiting for data
                    }
                    Err(crossbeam_channel::TryRecvError::Disconnected) => {
                        // The channel was disconnected, which is unexpected.
                        self.is_fetching = false;
                        self.fetch_status = "Error: Worker thread disconnected.".to_string();
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
                // Use a label to show the path, as it can be long
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

                    // --- Start background data fetching ---
                    let (sender, receiver) = crossbeam_channel::unbounded();
                    self.receiver = Some(receiver);

                    let api_key = self.api_key.clone();
                    let imdb_link = self.imdb_link.clone();
                    let series_dir = self.series_directory.clone();

                    std::thread::spawn(move || {
                        // --- 1. Scan for files (Moved inside the thread) ---
                        // This is a blocking operation, so it must be off the main UI thread.
                        let files: Vec<LocalFile> = WalkDir::new(series_dir)
                            .into_iter()
                            .filter_map(Result::ok)
                            .filter(|e| e.file_type().is_file())
                            .map(|e| LocalFile { path: e.into_path() })
                            .collect();

                        // --- 2. Fetch episodes (simplified for one season) ---
                        let imdb_id_result = imdb_link.split('/')
                            .find(|s| s.starts_with("tt"))
                            .map(String::from);

                        let imdb_id = match imdb_id_result {
                            Some(id) => id,
                            None => {
                                let _ = sender.send(AppMessage::FetchError("Could not find IMDb ID (e.g., 'tt123456') in the link.".to_string()));
                                return;
                            }
                        };

                        let request_url = format!("http://www.omdbapi.com/?i={}&Season=1&apikey={}", imdb_id, api_key);

                        let request = ehttp::Request::get(request_url);

                        ehttp::fetch(request, move |result: ehttp::Result<ehttp::Response>| {
                            match result {
                                Ok(response) => {
                                    if response.ok {
                                        match serde_json::from_slice::<SeasonResponse>(&response.bytes) {
                                            Ok(season_response) => {
                                                let _ = sender.send(AppMessage::DataFetched(season_response.episodes, files));
                                            }
                                            Err(e) => {
                                                let _ = sender.send(AppMessage::FetchError(format!("JSON Parse Error: {}", e)));
                                            }
                                        }
                                    } else {
                                        let _ = sender.send(AppMessage::FetchError(format!("API Error: {} {}", response.status, response.status_text)));
                                    }
                                }
                                Err(e) => {
                                    let _ = sender.send(AppMessage::FetchError(format!("Network Error: {}", e)));
                                }
                            }
                        });
                    });
                } else {
                    self.fetch_status = "Please provide both an IMDb link and a directory.".to_string();
                }
            }
            ui.label(&self.fetch_status);
        });

        // --- Processing Window ---
        let mut close_window = false;
        if self.show_process_window {
            egui::Window::new("Processing Window")
                .open(&mut self.show_process_window)
                .vscroll(true)
                .default_size([800.0, 600.0])
                .show(ctx, |ui| {
                    if self.is_fetching {
                        ui.label("Fetching data...");
                        ui.spinner();
                    } else if !self.episodes.is_empty() || !self.files.is_empty() {
                        // We will build the drag-and-drop UI here in the next stage.
                        ui.label("Ready for drag and drop!");
                    } else {
                        ui.label(&self.fetch_status);
                    }

                    if ui.button("Close").clicked() {
                        close_window = true;
                    }
                });
        }
        if close_window {
            self.show_process_window = false;
        }

        ctx.request_repaint();
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, eframe::APP_KEY, self);
    }
}
