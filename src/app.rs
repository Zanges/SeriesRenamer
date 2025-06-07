/// We derive Deserialize/Serialize so we can persist app state on shutdown.
#[derive(serde::Deserialize, serde::Serialize)]
#[serde(default)] // if we add new fields, give them default values when deserializing old state
pub struct SeriesRenamer {
    // Example stuff:
    pub imdb_link: String,
    pub series_directory: String,
}

impl Default for SeriesRenamer {
    fn default() -> Self {
        Self {
            imdb_link: String::new(),
            series_directory: String::new(),
        }
    }
}

impl SeriesRenamer {
    /// Called once before the first frame.
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // This is also where you can customize the look and feel of egui using
        // `cc.egui_ctx.set_visuals` and `cc.egui_ctx.set_fonts`.

        // Load previous app state (if any).
        // Note that you must enable the `persistence` feature for this to work.
        if let Some(storage) = cc.storage {
            return eframe::get_value(storage, eframe::APP_KEY).unwrap_or_default();
        }

        Default::default()
    }
}

impl eframe::App for SeriesRenamer {
    /// Called each time the UI needs repainting, which may be many times per second.
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Use the `egui::CentralPanel` to create a central area in the window.
        egui::CentralPanel::default().show(ctx, |ui| {
            // IMDB link input field
            ui.label("Enter IMDB link:");
            ui.text_edit_singleline(&mut self.imdb_link);
            
            // filebrowser to select the directory
            ui.label("Select directory:");
            if ui.button("Browse").clicked() {
                if let Some(path) = rfd::FileDialog::new().pick_folder() {
                    // Handle the selected directory path
                    self.series_directory = path.to_string_lossy().to_string();
                }
            }
            
            // process button
            if ui.button("Process").clicked() {
                // open a new window
                if !self.imdb_link.is_empty() && !self.series_directory.is_empty() {
                    // Here you would add the logic to process the IMDB link and series directory.
                    // For now, we will just print them to the console.
                    println!("IMDB Link: {}", self.imdb_link);
                    println!("Series Directory: {}", self.series_directory);
                } else {
                    ui.label("Please enter both IMDB link and select a directory.");
                }
            }
        });

        // Request a repaint if necessary.
        ctx.request_repaint();
    }

    /// Called by the frame work to save state before shutdown.
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, eframe::APP_KEY, self);
    }
}