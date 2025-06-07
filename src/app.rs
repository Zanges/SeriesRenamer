use serde::Deserialize;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

mod settings;

enum AppMessage {
    DataFetched(Vec<Episode>, Vec<LocalFile>),
    FetchError(String),
}

#[derive(Debug, Clone)]
pub struct LocalFile {
    pub path: PathBuf,
}

#[derive(Debug, Deserialize, Default)]
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

/// We derive Deserialize/Serialize so we can persist app state on shutdown.
#[derive(serde::Deserialize, serde::Serialize)]
#[serde(default)] // if we add new fields, give them default values when deserializing old state
pub struct SeriesRenamer {
    // Example stuff:
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
    /// Called once before the first frame.
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
    /// Called each time the UI needs repainting, which may be many times per second.
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.is_fetching {
            if let Some(rx) = &self.receiver {
                match rx.try_recv() {
                    Ok(AppMessage::DataFetched(episodes, files)) => {
                        self.episodes = episodes;
                        self.files = files;
                        self.is_fetching = false;
                        self.fetch_status = format!("Fetched {} episodes and {} files.", self.episodes.len(), self.files.len());

                        // --- FOR DEBUGGING ---
                        // This proves our data fetching works.
                        // We will remove this later.
                        println!("Episodes: {:#?}", self.episodes);
                        println!("Files: {:#?}", self.files);

                    }
                    Ok(AppMessage::FetchError(err_msg)) => {
                        self.is_fetching = false;
                        self.fetch_status = err_msg;
                    }
                    Err(_) => {
                        // Still waiting for data
                        ctx.request_repaint(); // Keep checking
                    }
                }
            }
        }
        
        // Use the `egui::CentralPanel` to create a central area in the window.
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Series Renamer");
            ui.separator();

            ui.horizontal(|ui| {
                ui.label("IMDb Link:");
                ui.text_edit_singleline(&mut self.imdb_link);
            });

            ui.horizontal(|ui| {
                ui.label("Series Directory:");
                ui.text_edit_singleline(&mut self.series_directory);
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

                    // --- Start background data fetching ---
                    let (sender, receiver) = crossbeam_channel::unbounded();
                    self.receiver = Some(receiver);

                    let api_key = self.api_key.clone();
                    let imdb_link = self.imdb_link.clone();
                    let series_dir = self.series_directory.clone();
                    let ehttp_ctx = ctx.clone();

                    std::thread::spawn(move || {
                        // --- 1. Scan for files ---
                        let files = WalkDir::new(series_dir)
                            .into_iter()
                            .filter_map(Result::ok)
                            .filter(|e| e.file_type().is_file())
                            .map(|e| LocalFile { path: e.into_path() })
                            .collect();

                        // --- 2. Fetch episodes (simplified for one season) ---
                        // A real implementation needs to get the IMDb ID from the link,
                        // then fetch total seasons, then loop to fetch each season.
                        // For now, we'll hardcode one season as a proof-of-concept.
                        let imdb_id = imdb_link.split('/').filter(|s| s.starts_with("tt")).next().unwrap_or("");
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
                }
            }
        });
        
        let mut process_window_close_button_clicked = false;
        if self.show_process_window {
            egui::Window::new("Processing Window")
                .open(&mut self.show_process_window)
                .vscroll(true)
                .default_size([800.0, 600.0])
                .show(ctx, |ui| {
                    ui.label(&self.fetch_status);
                    if self.is_fetching {
                        ui.spinner();
                    } else {
                        // We will build the drag-and-drop UI here in the next stage.
                        ui.label("Ready for drag and drop!");
                    }
                    if ui.button("Close").clicked() {
                        process_window_close_button_clicked = true;
                    }
                });
        }
        
        if process_window_close_button_clicked {
            self.show_process_window = false;
        }

        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }

    /// Called by the frame work to save state before shutdown.
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, eframe::APP_KEY, self);
    }
}