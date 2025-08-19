use anyhow::Result;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::get,
    Router,
};
use rusqlite::Connection;
use tower_http::services::ServeDir;
use serde::Serialize;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
struct AppState {
    db: Arc<Mutex<Connection>>,
}

#[derive(Serialize)]
struct BucketSummary {
    name: String,
    region: String,
    details: Value,
}

pub async fn start_server(db_path: PathBuf, listen_addr: String) -> Result<()> {
    let state = AppState {
        db: Arc::new(Mutex::new(Connection::open(db_path)?)),
    };

    let app = Router::new()
        .route("/api/s3_buckets", get(get_s3_buckets))
        .nest_service("/", ServeDir::new("static"))
        .with_state(state);

    println!("Starting server, listening on http://{}", listen_addr);
    let listener = tokio::net::TcpListener::bind(listen_addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

async fn get_s3_buckets(State(state): State<AppState>) -> impl IntoResponse {
    let conn = state.db.lock().unwrap();
    let mut stmt = match conn.prepare("SELECT name, region, details FROM resources WHERE resource_type = 's3:bucket' ORDER BY name") {
        Ok(stmt) => stmt,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    let bucket_iter = match stmt.query_map([], |row| {
        let details_str: String = row.get(2)?;
        let details: Value = serde_json::from_str(&details_str).unwrap_or_default();
        Ok(BucketSummary {
            name: row.get(0)?,
            region: row.get(1)?,
            details,
        })
    }) {
        Ok(iter) => iter,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    let buckets: Vec<BucketSummary> = bucket_iter.filter_map(Result::ok).collect();
    Json(buckets).into_response()
}
