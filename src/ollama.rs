// SPDX-License-Identifier: AGPL-3.0-or-later
//! Ollama on nvda11-gpu: the model reads the task, the student's work and the analyzer's findings and
//! proposes a mark with reasons. It does NOT see the AI-likelihood score — that flag must never colour a mark.

use anyhow::Result;
use serde::Deserialize;

pub struct Client {
    http: reqwest::Client,
    base: String,
}

#[derive(Debug, Deserialize)]
pub struct Proposal {
    pub mark: f64,
    #[serde(default)]
    pub rationale: String,
    #[serde(skip)]
    pub model: String,
}

impl Client {
    pub fn new(cfg: &crate::config::Config) -> Self {
        Self {
            http: reqwest::Client::builder().timeout(std::time::Duration::from_secs(600)).build().unwrap(),
            base: cfg.ollama_url.clone(),
        }
    }

    pub async fn ping(&self) -> bool {
        self.http.get(format!("{}/api/tags", self.base)).send().await.map(|r| r.status().is_success()).unwrap_or(false)
    }

    /// Ask for a rubric-based mark in JSON. Phase 1 uses a generic rubric; phase 2 passes the assignment's own
    /// "Yang dikumpulkan"/"Penilaian" sections and the question's points.
    pub async fn propose_mark(&self, cfg: &crate::config::Config, item: &crate::store::Item, a: &crate::analyzer::Analysis) -> Result<Proposal> {
        let prompt = format!(
            "Anda asisten penilai mata kuliah. Nilai jawaban mahasiswa untuk \"{title}\" pada skala 0-100.\n\
             Kriteria: (1) menjawab yang diminta, (2) kebenaran isi sesuai materi kuliah, (3) kelengkapan yang diminta, \
             (4) kejelasan. Kesesuaian dengan soal (0-1): {rel:.2}.{mat}\n\
             Jawaban mahasiswa:\n---\n{text}\n---\n\
             Balas HANYA JSON: {{\"mark\": angka 0-100, \"rationale\": \"alasan singkat berbahasa Indonesia, sebut bagian yang kurang\"}}",
            title = item.title, rel = a.relevance,
            // An unmeasured signal (0) must not reach the model, or it will mark the work down for it.
            mat = if a.material_match > 0.0 { format!(" Kesesuaian dengan materi kuliah (0-1): {:.2}.", a.material_match) } else { String::new() },
            text = a.text.chars().take(12_000).collect::<String>(),
        );
        let body = serde_json::json!({
            "model": cfg.model, "prompt": prompt, "stream": false, "format": "json",
            "options": { "temperature": 0.1 }, "think": false,
        });
        let v: serde_json::Value = self.http.post(format!("{}/api/generate", self.base)).json(&body).send().await?.json().await?;
        let raw = v["response"].as_str().unwrap_or("{}");
        let mut p: Proposal = serde_json::from_str(raw).unwrap_or(Proposal { mark: 0.0, rationale: format!("model tidak mengembalikan JSON: {raw}"), model: String::new() });
        p.mark = p.mark.clamp(0.0, 100.0);
        p.model = cfg.model.clone();
        Ok(p)
    }
}
