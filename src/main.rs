// SPDX-License-Identifier: AGPL-3.0-or-later
//! lms-grader — grading assistant for the hendrikarisma.my.id LMS, running on nvda11-gpu.
//!
//! Pipeline (see README.md):
//!   sync   pull courses → quizzes/attempts and assignments/submissions through `/api/v1` into a local SQLite
//!   grade  for every item without a proposal: run the Python analyzer (text extraction, similarity to the
//!          question and to the meeting's material, AI-likelihood, cited sources) and ask the Ollama model for
//!          a rubric-based mark; store a PROPOSAL
//!   serve  local review dashboard (127.0.0.1) — the lecturer accepts or edits proposals; only accepted marks
//!          are written back, and students see nothing until the marks are released in the LMS
//!
//! Two rules from the lecturer are structural, not settings: a proposal is never a released mark, and the
//! AI-likelihood score is a flag shown to the lecturer — it never changes a mark.

mod analyzer;
mod config;
mod lms;
mod ollama;
mod store;

use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
    let cfg = config::Config::from_env()?;
    let db = store::open(&cfg.db_path).await?;
    match std::env::args().nth(1).as_deref() {
        Some("sync") => {
            let lms = lms::Client::new(&cfg)?;
            let n = store::sync(&db, &lms).await?;
            println!("sync selesai: {n} item (kuis + tugas) di penyimpanan lokal");
        }
        Some("grade") => {
            let limit = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(20);
            let n = grade_pending(&cfg, &db, limit).await?;
            println!("{n} usulan nilai dibuat (belum dikirim ke LMS — tinjau dulu di dashboard)");
        }
        Some("serve") => serve(&cfg, db).await?,
        Some("check") => {
            let ok = ollama::Client::new(&cfg).ping().await;
            println!("ollama {}: {}", cfg.ollama_url, if ok { "ok" } else { "TIDAK TERJANGKAU" });
            println!("python analyzer: {}", analyzer::self_test(&cfg).await.unwrap_or_else(|e| format!("GAGAL: {e}")));
            println!("lms token: {}", if cfg.lms_token.is_empty() { "BELUM DIISI (.env LMS_TOKEN)" } else { "ada" });
        }
        _ => {
            eprintln!("pemakaian: lms-grader <check|sync|grade [N]|serve>");
            std::process::exit(2);
        }
    }
    Ok(())
}

/// Propose marks for up to `limit` items that have none yet.
async fn grade_pending(cfg: &config::Config, db: &sqlx::SqlitePool, limit: i64) -> Result<usize> {
    let llm = ollama::Client::new(cfg);
    let mut made = 0;
    for item in store::pending(db, limit).await? {
        let analysis = analyzer::analyze(cfg, &item).await?;
        let proposal = llm.propose_mark(cfg, &item, &analysis).await?;
        store::save_proposal(db, &item, &analysis, &proposal).await?;
        made += 1;
    }
    Ok(made)
}

/// Review dashboard — initial skeleton: lists proposals. Accept/edit/write-back come next (README, phase 3).
async fn serve(cfg: &config::Config, db: sqlx::SqlitePool) -> Result<()> {
    use axum::{extract::State, response::Html, routing::get, Router};
    async fn index(State(db): State<sqlx::SqlitePool>) -> Html<String> {
        let rows = store::proposals(&db).await.unwrap_or_default();
        let body: String = rows.iter().map(|p| format!(
            "<tr><td>{}</td><td>{}</td><td>{:.1}</td><td>{:.0}%</td><td>{}</td></tr>",
            html(&p.title), html(&p.student), p.mark, p.ai_likelihood * 100.0, html(&p.status),
        )).collect();
        Html(format!(
            "<!doctype html><meta charset=utf-8><title>LMS grader</title>\
             <h1>Usulan nilai</h1><p>Usulan belum dikirim ke LMS. Skor AI hanya tanda, tidak memotong nilai.</p>\
             <table border=1 cellpadding=4><tr><th>Item</th><th>Mahasiswa</th><th>Usulan</th><th>AI?</th><th>Status</th></tr>{body}</table>"
        ))
    }
    let app = Router::new().route("/", get(index)).with_state(db);
    let listener = tokio::net::TcpListener::bind(&cfg.bind).await?;
    println!("dashboard: http://{}", cfg.bind);
    axum::serve(listener, app).await?;
    Ok(())
}

fn html(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}
