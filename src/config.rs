// SPDX-License-Identifier: AGPL-3.0-or-later
//! Settings from the environment (and `.env` next to the binary's working directory).

use anyhow::Result;

pub struct Config {
    pub lms_base: String,
    pub lms_token: String,
    pub ollama_url: String,
    pub model: String,
    pub embed_model: String,
    pub db_path: String,
    pub bind: String,
    pub python: String,
    /// Required when `bind` is not loopback: HTTP Basic password for user "dosen".
    pub dash_password: String,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        load_dotenv(".env");
        let get = |k: &str, d: &str| std::env::var(k).unwrap_or_else(|_| d.to_string());
        Ok(Self {
            lms_base: get("LMS_BASE", "https://hendrikarisma.my.id").trim_end_matches('/').to_string(),
            lms_token: get("LMS_TOKEN", ""),
            ollama_url: get("OLLAMA_URL", "http://127.0.0.1:11434").trim_end_matches('/').to_string(),
            model: get("GRADER_MODEL", "qwen3:8b"),
            embed_model: get("EMBED_MODEL", "bge-m3"),
            db_path: get("GRADER_DB", "data/grader.db"),
            // Loopback only: the dashboard shows student work and is not meant for the network.
            bind: get("GRADER_BIND", "127.0.0.1:8431"),
            python: get("GRADER_PYTHON", "python/.venv/bin/python"),
            dash_password: get("GRADER_DASH_PASSWORD", ""),
        })
    }
}

/// Minimal KEY=VALUE loader; existing environment variables win.
fn load_dotenv(path: &str) {
    let Ok(text) = std::fs::read_to_string(path) else { return };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') { continue; }
        if let Some((k, v)) = line.split_once('=') {
            if std::env::var(k.trim()).is_err() {
                std::env::set_var(k.trim(), v.trim().trim_matches('"'));
            }
        }
    }
}
