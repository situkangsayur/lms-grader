// SPDX-License-Identifier: AGPL-3.0-or-later
//! Bridge to the Python analyzer (`python/analyzer.py`): one JSON object in on stdin, one out on stdout.
//! Python does what its libraries do best — reading PDF/DOCX/ipynb, embeddings maths, the ONNX AI detector.

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Analysis {
    /// Text the student handed in (extracted from files and essay answers), truncated for the model.
    #[serde(default)]
    pub text: String,
    /// Cosine similarity between the work and the task/question, 0..1.
    #[serde(default)]
    pub relevance: f64,
    /// Cosine similarity between the work and the meeting's slide text, 0..1.
    #[serde(default)]
    pub material_match: f64,
    /// Probability the text is machine-written, 0..1. A flag for the lecturer only.
    #[serde(default)]
    pub ai_likelihood: f64,
    /// URLs and references found in the work.
    #[serde(default)]
    pub sources: Vec<String>,
    #[serde(default)]
    pub notes: Vec<String>,
}

pub async fn analyze(cfg: &crate::config::Config, item: &crate::store::Item) -> Result<Analysis> {
    let req = serde_json::json!({
        "op": "analyze", "kind": item.kind, "title": item.title, "payload": item.payload,
        "ollama_url": cfg.ollama_url, "embed_model": cfg.embed_model,
    });
    let out = run(cfg, &req).await?;
    Ok(serde_json::from_value(out)?)
}

pub async fn self_test(cfg: &crate::config::Config) -> Result<String> {
    let out = run(cfg, &serde_json::json!({ "op": "selftest" })).await?;
    Ok(out.to_string())
}

async fn run(cfg: &crate::config::Config, req: &serde_json::Value) -> Result<serde_json::Value> {
    let mut child = tokio::process::Command::new(&cfg.python)
        .arg("python/analyzer.py")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;
    child.stdin.take().unwrap().write_all(req.to_string().as_bytes()).await?;
    let out = child.wait_with_output().await?;
    if !out.status.success() {
        bail!("analyzer gagal: {}", String::from_utf8_lossy(&out.stderr));
    }
    Ok(serde_json::from_slice(&out.stdout)?)
}
