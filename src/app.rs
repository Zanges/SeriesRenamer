/// We derive Deserialize/Serialize so we can persist app state on shutdown.
#[derive(serde::Deserialize, serde::Serialize)]
#[serde(default)] // if we add new fields, give them default values when deserializing old state
pub struct SeriesRenamer {
    // Example stuff:
    pub imdb_link: String,
    pub series_directory: String,
    pub show_process_window: bool,
}

impl Default for SeriesRenamer {
    fn default() -> Self {
        Self {
            imdb_link: String::new(),
            series_directory: String::new(),
            show_process_window: false,
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
                    self.show_process_window = true;
                } else {
                    ui.label("Please enter both IMDB link and select a directory.");
                }
            }
        });
        
        let mut process_window_close_button_clicked = false;
        if self.show_process_window {
            // `egui::Window::new` creates a new window.
            // The `.open()` method provides a close button and binds visibility to our boolean.
            egui::Window::new("Processing Window")
                .open(&mut self.show_process_window) // Binds the window's visibility to self.show_process_window
                .vscroll(true)
                .show(ctx, |ui| {
                    // Add the content for your new window here.
                    ui.label("Processing the following:");
                    ui.separator();
                    ui.label(format!("IMDB Link: {}", self.imdb_link));
                    ui.label(format!("Series Directory: {}", self.series_directory));

                    if ui.button("Close").clicked() {
                        process_window_close_button_clicked = true;
                    }
                });
        }
        
        if process_window_close_button_clicked {
            self.show_process_window = false;
        }

        // Request a repaint if necessary.
        ctx.request_repaint();
    }

    /// Called by the frame work to save state before shutdown.
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, eframe::APP_KEY, self);
    }
}