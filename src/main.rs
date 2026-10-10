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
mod web;

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
        Some("serve") => web::serve(&cfg, db).await?,
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
