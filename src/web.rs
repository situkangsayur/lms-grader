// SPDX-License-Identifier: AGPL-3.0-or-later
//! Review dashboard. Server-rendered HTML, no JavaScript framework.
//!
//! Access: it shows students' work, so it refuses to listen on anything but loopback unless
//! `GRADER_DASH_PASSWORD` is set, and then every request needs HTTP Basic auth (user "dosen").
//! Decisions made here change only the local database; sending accepted marks to the LMS is phase 3.

use axum::{
    extract::{Form, Path, Query, Request, State},
    http::{header, StatusCode},
    middleware::{self, Next},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
    Router,
};
use sqlx::SqlitePool;
use std::collections::HashMap;

#[derive(Clone)]
struct App {
    db: SqlitePool,
    password: String,
}

pub async fn serve(cfg: &crate::config::Config, db: SqlitePool) -> anyhow::Result<()> {
    let loopback = cfg.bind.starts_with("127.") || cfg.bind.starts_with("localhost") || cfg.bind.starts_with("[::1]");
    if !loopback && cfg.dash_password.is_empty() {
        anyhow::bail!("GRADER_BIND={} bukan loopback: isi GRADER_DASH_PASSWORD dulu (dashboard berisi karya mahasiswa)", cfg.bind);
    }
    let app = App { db, password: cfg.dash_password.clone() };
    let router = Router::new()
        .route("/", get(overview))
        .route("/items", get(items))
        .route("/item/{key}", get(item))
        .route("/item/{key}/decide", post(decide))
        .layer(middleware::from_fn_with_state(app.clone(), auth))
        .with_state(app);
    let listener = tokio::net::TcpListener::bind(&cfg.bind).await?;
    println!("dashboard: http://{}", cfg.bind);
    axum::serve(listener, router).await?;
    Ok(())
}

async fn auth(State(app): State<App>, req: Request, next: Next) -> Response {
    if app.password.is_empty() {
        return next.run(req).await;
    }
    let ok = req.headers().get(header::AUTHORIZATION).and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Basic "))
        .and_then(|b| base64_decode(b))
        .map(|cred| cred == format!("dosen:{}", app.password))
        .unwrap_or(false);
    if ok { return next.run(req).await; }
    (StatusCode::UNAUTHORIZED, [(header::WWW_AUTHENTICATE, "Basic realm=\"lms-grader\"")], "login diperlukan").into_response()
}

async fn overview(State(app): State<App>) -> Html<String> {
    let s = crate::store::stats(&app.db).await.unwrap_or_default();
    let courses: String = s.by_course.iter().map(|(c, n, p)| format!(
        r#"<tr><td><a href="/items?course={c}">kursus {c}</a></td><td>{n}</td><td>{p}</td></tr>"#)).collect();
    Html(page("Ringkasan", &format!(
        r#"<div class="cards">
  <div class="card"><b>{items}</b><span>item tersinkron</span></div>
  <div class="card"><b>{pending}</b><span>belum diusulkan</span></div>
  <div class="card"><b>{proposed}</b><span>usulan menunggu tinjauan</span></div>
  <div class="card"><b>{decided}</b><span>sudah ditinjau</span></div>
  <div class="card warn"><b>{flagged}</b><span>tanda AI ≥ 0,7</span></div>
</div>
<p class="muted">Terakhir sinkron: {last}. Jalankan <code>lms-grader sync</code> lalu <code>lms-grader grade</code> untuk mengisi usulan.</p>
<h2>Per mata kuliah</h2>
<table><tr><th>Mata kuliah</th><th>Item</th><th>Usulan</th></tr>{courses}</table>
<p><a class="btn" href="/items?status=proposed">Tinjau usulan →</a></p>"#,
        items = s.items, pending = s.pending, proposed = s.proposed, decided = s.decided, flagged = s.flagged,
        last = if s.last_sync.is_empty() { "belum pernah".into() } else { s.last_sync.clone() },
        courses = if courses.is_empty() { r#"<tr><td colspan="3" class="muted">Belum ada data.</td></tr>"#.to_string() } else { courses },
    )))
}

async fn items(State(app): State<App>, Query(q): Query<HashMap<String, String>>) -> Html<String> {
    let status = q.get("status").cloned().unwrap_or_default();
    let course = q.get("course").and_then(|c| c.parse::<i64>().ok());
    let rows = crate::store::list(&app.db, &status, course).await.unwrap_or_default();
    let filt = |v: &str, label: &str| format!(
        r#"<a class="chip{a}" href="/items?status={v}{c}">{label}</a>"#,
        a = if status == v { " on" } else { "" }, c = course.map(|c| format!("&course={c}")).unwrap_or_default());
    let body: String = rows.iter().map(|r| format!(
        r#"<tr><td><a href="/item/{key}">{title}</a><div class="muted">{kind} · {student}</div></td>
<td class="num">{mark}</td><td class="num">{rel}</td><td class="num {aic}">{ai}</td><td><span class="pill {st}">{st}</span></td></tr>"#,
        key = esc(&r.key), title = esc(&r.title), kind = esc(&r.kind), student = esc(&r.student),
        mark = r.mark.map(|m| format!("{m:.0}")).unwrap_or_else(|| "—".into()),
        rel = r.relevance.map(|v| format!("{v:.2}")).unwrap_or_else(|| "—".into()),
        ai = r.ai.map(|v| format!("{:.0}%", v * 100.0)).unwrap_or_else(|| "—".into()),
        aic = if r.ai.unwrap_or(0.0) >= 0.7 { "flag" } else { "" },
        st = esc(&r.status),
    )).collect();
    Html(page("Usulan nilai", &format!(
        r#"<p>{a}{b}{c}{d}</p>
<table><tr><th>Item</th><th>Usulan</th><th>Kesesuaian soal</th><th>Tanda AI</th><th>Status</th></tr>{body}</table>"#,
        a = filt("", "semua"), b = filt("pending", "belum diusulkan"), c = filt("proposed", "menunggu tinjauan"), d = filt("accepted", "disetujui"),
        body = if body.is_empty() { r#"<tr><td colspan="5" class="muted">Tidak ada item.</td></tr>"#.to_string() } else { body },
    )))
}

async fn item(State(app): State<App>, Path(key): Path<String>) -> Response {
    let Some(d) = crate::store::detail(&app.db, &key).await.ok().flatten() else {
        return (StatusCode::NOT_FOUND, Html(page("Tidak ditemukan", "<p>Item tidak ada.</p>"))).into_response();
    };
    let sources: String = d.sources.iter().map(|s| format!(r#"<li><a href="{u}" rel="noreferrer" target="_blank">{u}</a></li>"#, u = esc(s))).collect();
    let proposal = match d.mark {
        Some(m) => format!(
            r#"<div class="panel">
  <h2>Usulan model: {m:.0} / 100</h2><p>{rat}</p><p class="muted">model {model} · status <b>{st}</b></p>
  <form method="post" action="/item/{key}/decide" class="row">
    <label>Nilai akhir <input name="mark" type="number" step="0.5" min="0" max="100" value="{final_v}"></label>
    <label class="grow">Catatan untuk mahasiswa <input name="note" value="{note}"></label>
    <button name="do" value="accept">Setujui</button>
    <button name="do" value="reset" class="ghost">Kembalikan ke usulan</button>
  </form>
  <p class="muted">Persetujuan disimpan di sini. Pengiriman ke LMS dan rilis nilai ke mahasiswa adalah langkah terpisah (fase 3).</p>
</div>"#,
            rat = esc(&d.rationale), model = esc(&d.model), st = esc(&d.status), key = esc(&d.key),
            final_v = d.final_mark.unwrap_or(m), note = esc(&d.note)),
        None => r#"<div class="panel"><p class="muted">Belum ada usulan. Jalankan <code>lms-grader grade</code>.</p></div>"#.to_string(),
    };
    Html(page(&d.title, &format!(
        r#"<p><a href="/items">← daftar</a></p>
<p class="muted">{kind} · mahasiswa {student} · {key}</p>
<div class="grid">
  <div class="panel"><h2>Kesesuaian dengan soal</h2><p class="big">{rel}</p></div>
  <div class="panel"><h2>Kesesuaian dengan materi</h2><p class="big">{mat}</p></div>
  <div class="panel {aic}"><h2>Tanda AI</h2><p class="big">{ai}</p><p class="muted">hanya tanda — tidak mengubah nilai</p></div>
</div>
{proposal}
<div class="panel"><h2>Sumber yang dipakai</h2>{src}</div>
<div class="panel"><h2>Karya mahasiswa (teks yang dibaca model)</h2><pre>{text}</pre></div>
<div class="panel"><h2>Catatan analyzer</h2><ul>{notes}</ul></div>"#,
        kind = esc(&d.kind), student = esc(&d.student), key = esc(&d.key),
        rel = d.relevance.map(|v| format!("{v:.2}")).unwrap_or_else(|| "—".into()),
        mat = d.material_match.filter(|v| *v > 0.0).map(|v| format!("{v:.2}")).unwrap_or_else(|| "belum diukur".into()),
        ai = d.ai.filter(|v| *v > 0.0).map(|v| format!("{:.0}%", v * 100.0)).unwrap_or_else(|| "belum diukur".into()),
        aic = if d.ai.unwrap_or(0.0) >= 0.7 { "flag" } else { "" },
        proposal = proposal,
        src = if sources.is_empty() { r#"<p class="muted">Tidak ada tautan/DOI.</p>"#.to_string() } else { format!("<ul>{sources}</ul>") },
        text = esc(&d.text), notes = d.notes.iter().map(|n| format!("<li>{}</li>", esc(n))).collect::<String>(),
    ))).into_response()
}

async fn decide(State(app): State<App>, Path(key): Path<String>, Form(f): Form<HashMap<String, String>>) -> Response {
    let mark = f.get("mark").and_then(|m| m.replace(',', ".").parse::<f64>().ok()).map(|m| m.clamp(0.0, 100.0));
    let note = f.get("note").cloned().unwrap_or_default();
    let reset = f.get("do").map(|d| d == "reset").unwrap_or(false);
    let _ = crate::store::decide(&app.db, &key, if reset { None } else { mark }, &note, reset).await;
    Redirect::to(&format!("/item/{key}")).into_response()
}

fn page(title: &str, body: &str) -> String {
    format!(r#"<!doctype html><html lang="id"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>{t} · lms-grader</title><style>{css}</style>
<header><a href="/"><b>lms-grader</b></a><nav><a href="/">Ringkasan</a><a href="/items?status=proposed">Usulan</a><a href="/items">Semua</a></nav></header>
<main><h1>{t}</h1>{body}</main>
<footer>Usulan bukan nilai: dosen menyetujui, lalu merilis di LMS. Tanda AI tidak pernah memotong nilai. · AGPL-3.0-or-later</footer></html>"#,
        t = esc(title), css = CSS, body = body)
}

const CSS: &str = r#"
:root{--ink:#15291d;--dim:#5a6f62;--line:#d9e3dc;--bg:#f6faf6;--card:#fff;--acc:#1f9d55;--warn:#b45309}
@media (prefers-color-scheme:dark){:root{--ink:#e8eef7;--dim:#9fb2c7;--line:#2a3646;--bg:#0f1622;--card:#151f2e;--acc:#19c06a;--warn:#f59e0b}}
*{box-sizing:border-box}body{margin:0;font:15px/1.55 system-ui,sans-serif;color:var(--ink);background:var(--bg)}
header{display:flex;gap:20px;align-items:center;padding:12px 20px;border-bottom:1px solid var(--line);background:var(--card)}
header a{color:var(--ink);text-decoration:none}nav{display:flex;gap:14px}nav a{color:var(--dim)}
main{max-width:1100px;margin:0 auto;padding:16px 20px}h1{font-size:24px}h2{font-size:15px;margin:0 0 6px}
a{color:var(--acc)}.muted{color:var(--dim);font-size:13px}code{background:var(--line);padding:1px 5px;border-radius:4px}
table{width:100%;border-collapse:collapse;background:var(--card);border:1px solid var(--line);border-radius:10px;overflow:hidden}
th,td{padding:8px 10px;border-bottom:1px solid var(--line);text-align:left;vertical-align:top}th{font-size:12px;color:var(--dim);text-transform:uppercase}
.num{text-align:right;white-space:nowrap}.flag{color:var(--warn);font-weight:700}
.cards{display:grid;grid-template-columns:repeat(auto-fit,minmax(160px,1fr));gap:12px;margin:12px 0}
.card{background:var(--card);border:1px solid var(--line);border-radius:12px;padding:14px}.card b{display:block;font-size:28px}.card span{color:var(--dim);font-size:13px}
.card.warn b{color:var(--warn)}
.grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(220px,1fr));gap:12px}
.panel{background:var(--card);border:1px solid var(--line);border-radius:12px;padding:14px;margin:12px 0}.panel.flag{border-color:var(--warn)}
.big{font-size:26px;font-weight:700;margin:0}pre{white-space:pre-wrap;font:13px/1.5 ui-monospace,monospace;max-height:420px;overflow:auto}
.row{display:flex;flex-wrap:wrap;gap:10px;align-items:end}.row label{display:flex;flex-direction:column;font-size:12px;color:var(--dim)}.row .grow{flex:1;min-width:220px}
input{font:inherit;padding:6px 8px;border:1px solid var(--line);border-radius:8px;background:var(--bg);color:var(--ink)}
button,.btn{font:inherit;font-weight:600;padding:7px 14px;border-radius:8px;border:1px solid var(--acc);background:var(--acc);color:#fff;cursor:pointer;text-decoration:none;display:inline-block}
button.ghost{background:transparent;color:var(--acc)}
.chip{display:inline-block;margin-right:6px;padding:3px 10px;border:1px solid var(--line);border-radius:999px;font-size:13px;text-decoration:none;color:var(--dim)}.chip.on{background:var(--acc);color:#fff;border-color:var(--acc)}
.pill{font-size:12px;padding:2px 8px;border-radius:999px;border:1px solid var(--line)}
footer{max-width:1100px;margin:20px auto;padding:0 20px 20px;color:var(--dim);font-size:12px}
"#;

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Standard base64 → UTF-8 string (enough for an Authorization header; no extra crate).
fn base64_decode(s: &str) -> Option<String> {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::new();
    let (mut buf, mut bits) = (0u32, 0u32);
    for c in s.trim().bytes().filter(|c| *c != b'=') {
        let v = T.iter().position(|t| *t == c)? as u32;
        buf = (buf << 6) | v;
        bits += 6;
        if bits >= 8 { bits -= 8; out.push((buf >> bits) as u8 & 0xff); }
    }
    String::from_utf8(out).ok()
}
