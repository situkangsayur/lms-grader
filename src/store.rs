// SPDX-License-Identifier: AGPL-3.0-or-later
//! Local SQLite: what was pulled from the LMS, and the proposals made for it.
//! Nothing here is the LMS's truth — it is a working copy plus the grader's opinions.

use anyhow::Result;
use serde_json::Value;
use sqlx::{sqlite::SqliteConnectOptions, Row, SqlitePool};

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS items (
  key          TEXT PRIMARY KEY,            -- 'attempt:<id>' | 'submission:<id>'
  kind         TEXT NOT NULL,               -- 'quiz' | 'assignment'
  course_id    INTEGER NOT NULL,
  parent_id    INTEGER NOT NULL,            -- quiz id or assignment id
  title        TEXT NOT NULL DEFAULT '',
  student      TEXT NOT NULL DEFAULT '',    -- user id as text (resolved to NIM in phase 2)
  payload      TEXT NOT NULL,               -- raw JSON from the API
  pulled_at    INTEGER NOT NULL DEFAULT (strftime('%s','now'))
);
CREATE TABLE IF NOT EXISTS proposals (
  item_key       TEXT PRIMARY KEY REFERENCES items(key) ON DELETE CASCADE,
  mark           REAL NOT NULL,             -- proposed score, 0..100
  rationale      TEXT NOT NULL DEFAULT '',
  relevance      REAL NOT NULL DEFAULT 0,   -- similarity to the question/task, 0..1
  material_match REAL NOT NULL DEFAULT 0,   -- similarity to the meeting's slides, 0..1
  ai_likelihood  REAL NOT NULL DEFAULT 0,   -- FLAG ONLY, 0..1 — never applied to the mark
  sources        TEXT NOT NULL DEFAULT '[]',-- JSON list of sources the work cites or links
  model          TEXT NOT NULL DEFAULT '',
  status         TEXT NOT NULL DEFAULT 'proposed', -- proposed | accepted | edited | sent
  created_at     INTEGER NOT NULL DEFAULT (strftime('%s','now'))
);
"#;

pub async fn open(path: &str) -> Result<SqlitePool> {
    if let Some(dir) = std::path::Path::new(path).parent() { std::fs::create_dir_all(dir).ok(); }
    let opts = SqliteConnectOptions::new().filename(path).create_if_missing(true);
    let db = SqlitePool::connect_with(opts).await?;
    for stmt in SCHEMA.split(';').map(str::trim).filter(|s| !s.is_empty()) {
        sqlx::query(stmt).execute(&db).await?;
    }
    Ok(db)
}

/// Pull every quiz attempt and assignment submission the token may see.
pub async fn sync(db: &SqlitePool, lms: &crate::lms::Client) -> Result<usize> {
    let mut n = 0;
    let courses = lms.get("/courses").await?;
    for c in courses.get("courses").and_then(Value::as_array).cloned().unwrap_or_default() {
        let cid = c["id"].as_i64().unwrap_or(0);
        let assigns = lms.get(&format!("/courses/{cid}/assignments")).await?;
        for a in assigns.get("assignments").and_then(Value::as_array).cloned().unwrap_or_default() {
            let aid = a["id"].as_i64().unwrap_or(0);
            let subs = lms.get(&format!("/assignments/{aid}/submissions")).await?;
            for s in subs.get("submissions").and_then(Value::as_array).cloned().unwrap_or_default() {
                upsert(db, &format!("submission:{}", s["id"]), "assignment", cid, aid, a["title_id"].as_str().or(a["title_en"].as_str()).unwrap_or(""), &s["user_id"].to_string(), &s).await?;
                n += 1;
            }
        }
        let quizzes = lms.get(&format!("/courses/{cid}/quizzes")).await?;
        for q in quizzes.get("quizzes").and_then(Value::as_array).cloned().unwrap_or_default() {
            let qid = q["id"].as_i64().unwrap_or(0);
            let atts = lms.get(&format!("/quizzes/{qid}/attempts")).await?;
            for at in atts.get("attempts").and_then(Value::as_array).cloned().unwrap_or_default() {
                if at["submitted_at"].as_i64().unwrap_or(0) == 0 { continue; }
                upsert(db, &format!("attempt:{}", at["id"]), "quiz", cid, qid, q["title_id"].as_str().or(q["title_en"].as_str()).unwrap_or(""), &at["user_id"].to_string(), &at).await?;
                n += 1;
            }
        }
    }
    Ok(n)
}

async fn upsert(db: &SqlitePool, key: &str, kind: &str, cid: i64, parent: i64, title: &str, student: &str, payload: &Value) -> Result<()> {
    sqlx::query("INSERT INTO items (key,kind,course_id,parent_id,title,student,payload) VALUES (?,?,?,?,?,?,?) \
                 ON CONFLICT(key) DO UPDATE SET payload=excluded.payload, title=excluded.title, pulled_at=strftime('%s','now')")
        .bind(key).bind(kind).bind(cid).bind(parent).bind(title).bind(student).bind(payload.to_string())
        .execute(db).await?;
    Ok(())
}

pub struct Item {
    pub key: String,
    pub kind: String,
    pub title: String,
    pub payload: Value,
}

pub async fn pending(db: &SqlitePool, limit: i64) -> Result<Vec<Item>> {
    let rows = sqlx::query("SELECT key, kind, title, payload FROM items WHERE key NOT IN (SELECT item_key FROM proposals) ORDER BY pulled_at LIMIT ?")
        .bind(limit).fetch_all(db).await?;
    Ok(rows.into_iter().map(|r| Item {
        key: r.get("key"), kind: r.get("kind"), title: r.get("title"),
        payload: serde_json::from_str(r.get::<String, _>("payload").as_str()).unwrap_or(Value::Null),
    }).collect())
}

pub async fn save_proposal(db: &SqlitePool, item: &Item, a: &crate::analyzer::Analysis, p: &crate::ollama::Proposal) -> Result<()> {
    sqlx::query("INSERT OR REPLACE INTO proposals (item_key,mark,rationale,relevance,material_match,ai_likelihood,sources,model,status) \
                 VALUES (?,?,?,?,?,?,?,?,'proposed')")
        .bind(&item.key).bind(p.mark).bind(&p.rationale).bind(a.relevance).bind(a.material_match)
        .bind(a.ai_likelihood).bind(serde_json::to_string(&a.sources)?).bind(&p.model)
        .execute(db).await?;
    Ok(())
}

pub struct ProposalRow {
    pub title: String,
    pub student: String,
    pub mark: f64,
    pub ai_likelihood: f64,
    pub status: String,
}

pub async fn proposals(db: &SqlitePool) -> Result<Vec<ProposalRow>> {
    let rows = sqlx::query("SELECT i.title, i.student, p.mark, p.ai_likelihood, p.status FROM proposals p JOIN items i ON i.key=p.item_key ORDER BY p.created_at DESC")
        .fetch_all(db).await?;
    Ok(rows.into_iter().map(|r| ProposalRow {
        title: r.get("title"), student: r.get("student"), mark: r.get("mark"),
        ai_likelihood: r.get("ai_likelihood"), status: r.get("status"),
    }).collect())
}
