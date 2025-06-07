// src/settings.rs
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct AppSettings {
    pub api_key: String,
}

/// `confy` requires a default implementation.
impl Default for AppSettings {
    fn default() -> Self {
        Self {
            api_key: String::from("YOUR_API_KEY_HERE"),
        }
    }
}