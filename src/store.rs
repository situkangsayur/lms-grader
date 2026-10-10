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
  text           TEXT NOT NULL DEFAULT '',  -- what the model read
  notes          TEXT NOT NULL DEFAULT '[]',-- analyzer notes (what was not measured, and why)
  final_mark     REAL,                      -- the lecturer's decision; NULL = not reviewed
  note           TEXT NOT NULL DEFAULT '',  -- lecturer's feedback for the student
  status         TEXT NOT NULL DEFAULT 'proposed', -- proposed | accepted | sent
  created_at     INTEGER NOT NULL DEFAULT (strftime('%s','now'))
);
"#;

pub async fn open(path: &str) -> Result<SqlitePool> {
    if let Some(dir) = std::path::Path::new(path).parent() { std::fs::create_dir_all(dir).ok(); }
    let opts = SqliteConnectOptions::new().filename(path).create_if_missing(true);
    let db = SqlitePool::connect_with(opts).await?;
    // Strip `--` comments first: a ';' inside a comment must not split a statement.
    let schema: String = SCHEMA.lines().map(|l| l.split("--").next().unwrap_or("")).collect::<Vec<_>>().join("\n");
    for stmt in schema.split(';').map(str::trim).filter(|s| !s.is_empty()) {
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
    sqlx::query("INSERT OR REPLACE INTO proposals (item_key,mark,rationale,relevance,material_match,ai_likelihood,sources,model,text,notes,status) \
                 VALUES (?,?,?,?,?,?,?,?,?,?,'proposed')")
        .bind(&item.key).bind(p.mark).bind(&p.rationale).bind(a.relevance).bind(a.material_match)
        .bind(a.ai_likelihood).bind(serde_json::to_string(&a.sources)?).bind(&p.model)
        .bind(&a.text).bind(serde_json::to_string(&a.notes)?)
        .execute(db).await?;
    Ok(())
}

#[derive(Default)]
pub struct Stats {
    pub items: i64,
    pub pending: i64,
    pub proposed: i64,
    pub decided: i64,
    pub flagged: i64,
    pub last_sync: String,
    pub by_course: Vec<(i64, i64, i64)>,
}

pub async fn stats(db: &SqlitePool) -> Result<Stats> {
    let one = |sql: &'static str| async move { sqlx::query_scalar::<_, i64>(sql).fetch_one(db).await.unwrap_or(0) };
    let by_course = sqlx::query("SELECT i.course_id, count(*) n, count(p.item_key) p FROM items i LEFT JOIN proposals p ON p.item_key=i.key GROUP BY i.course_id ORDER BY i.course_id")
        .fetch_all(db).await?.into_iter().map(|r| (r.get::<i64, _>("course_id"), r.get::<i64, _>("n"), r.get::<i64, _>("p"))).collect();
    Ok(Stats {
        items: one("SELECT count(*) FROM items").await,
        pending: one("SELECT count(*) FROM items WHERE key NOT IN (SELECT item_key FROM proposals)").await,
        proposed: one("SELECT count(*) FROM proposals WHERE status='proposed'").await,
        decided: one("SELECT count(*) FROM proposals WHERE status IN ('accepted','sent')").await,
        flagged: one("SELECT count(*) FROM proposals WHERE ai_likelihood >= 0.7").await,
        last_sync: sqlx::query_scalar::<_, Option<String>>("SELECT datetime(max(pulled_at),'unixepoch','+7 hours') FROM items").fetch_one(db).await.ok().flatten().unwrap_or_default(),
        by_course,
    })
}

pub struct ListRow {
    pub key: String,
    pub kind: String,
    pub title: String,
    pub student: String,
    pub mark: Option<f64>,
    pub relevance: Option<f64>,
    pub ai: Option<f64>,
    pub status: String,
}

/// `status`: "" all · "pending" no proposal yet · otherwise the proposal status.
pub async fn list(db: &SqlitePool, status: &str, course: Option<i64>) -> Result<Vec<ListRow>> {
    let rows = sqlx::query(
        "SELECT i.key, i.kind, i.title, i.student, coalesce(p.final_mark, p.mark) mark, p.relevance, p.ai_likelihood, coalesce(p.status,'pending') status \
         FROM items i LEFT JOIN proposals p ON p.item_key=i.key \
         WHERE (?1='' OR coalesce(p.status,'pending')=?1) AND (?2 IS NULL OR i.course_id=?2) ORDER BY i.course_id, i.title, i.student LIMIT 1000")
        .bind(status).bind(course).fetch_all(db).await?;
    Ok(rows.into_iter().map(|r| ListRow {
        key: r.get("key"), kind: r.get("kind"), title: r.get("title"), student: r.get("student"),
        mark: r.get("mark"), relevance: r.get("relevance"), ai: r.get("ai_likelihood"), status: r.get("status"),
    }).collect())
}

pub struct Detail {
    pub key: String,
    pub kind: String,
    pub title: String,
    pub student: String,
    pub mark: Option<f64>,
    pub final_mark: Option<f64>,
    pub rationale: String,
    pub relevance: Option<f64>,
    pub material_match: Option<f64>,
    pub ai: Option<f64>,
    pub sources: Vec<String>,
    pub notes: Vec<String>,
    pub text: String,
    pub model: String,
    pub note: String,
    pub status: String,
}

pub async fn detail(db: &SqlitePool, key: &str) -> Result<Option<Detail>> {
    let Some(r) = sqlx::query(
        "SELECT i.key, i.kind, i.title, i.student, p.mark, p.final_mark, coalesce(p.rationale,'') rationale, p.relevance, p.material_match, \
         p.ai_likelihood, coalesce(p.sources,'[]') sources, coalesce(p.notes,'[]') notes, coalesce(p.text,'') text, coalesce(p.model,'') model, \
         coalesce(p.note,'') note, coalesce(p.status,'pending') status FROM items i LEFT JOIN proposals p ON p.item_key=i.key WHERE i.key=?")
        .bind(key).fetch_optional(db).await? else { return Ok(None) };
    Ok(Some(Detail {
        key: r.get("key"), kind: r.get("kind"), title: r.get("title"), student: r.get("student"),
        mark: r.get("mark"), final_mark: r.get("final_mark"), rationale: r.get("rationale"),
        relevance: r.get("relevance"), material_match: r.get("material_match"), ai: r.get("ai_likelihood"),
        sources: serde_json::from_str(r.get::<String, _>("sources").as_str()).unwrap_or_default(),
        notes: serde_json::from_str(r.get::<String, _>("notes").as_str()).unwrap_or_default(),
        text: r.get("text"), model: r.get("model"), note: r.get("note"), status: r.get("status"),
    }))
}

/// Record the lecturer's decision locally. `reset` returns the item to the model's proposal.
pub async fn decide(db: &SqlitePool, key: &str, mark: Option<f64>, note: &str, reset: bool) -> Result<()> {
    if reset {
        sqlx::query("UPDATE proposals SET final_mark=NULL, note='', status='proposed' WHERE item_key=?").bind(key).execute(db).await?;
    } else {
        sqlx::query("UPDATE proposals SET final_mark=?, note=?, status='accepted' WHERE item_key=?").bind(mark).bind(note).bind(key).execute(db).await?;
    }
    Ok(())
}
