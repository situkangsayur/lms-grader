// SPDX-License-Identifier: AGPL-3.0-or-later
//! Client for the LMS JSON API (`/api/v1`, bearer token, staff scope).
//!
//! Endpoints used (all exist today): /me, /courses, /courses/{id}/assignments, /assignments/{id}/submissions,
//! /courses/{id}/quizzes, /quizzes/{id}/attempts. Still missing in the LMS and needed for phase 2:
//! downloading a submission file and reading quiz questions/options — see README "Yang perlu ditambah di LMS".

use anyhow::{bail, Result};
use serde_json::Value;

pub struct Client {
    http: reqwest::Client,
    base: String,
    token: String,
}

impl Client {
    pub fn new(cfg: &crate::config::Config) -> Result<Self> {
        if cfg.lms_token.is_empty() {
            bail!("LMS_TOKEN kosong — buat token di /admin/api (akun staf, scope write) dan isi .env");
        }
        Ok(Self {
            http: reqwest::Client::builder().timeout(std::time::Duration::from_secs(60)).build()?,
            base: format!("{}/api/v1", cfg.lms_base),
            token: cfg.lms_token.clone(),
        })
    }

    pub async fn get(&self, path: &str) -> Result<Value> {
        let r = self.http.get(format!("{}{}", self.base, path)).bearer_auth(&self.token).send().await?;
        let status = r.status();
        let body: Value = r.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            bail!("GET {path}: {status} {body}");
        }
        Ok(body)
    }
}
