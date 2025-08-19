use anyhow::Result;
use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::get,
    Router,
};
use serde::Deserialize;
use tower_http::services::ServeDir;
use std::{path::PathBuf, sync::Arc};

use crate::query;

#[derive(Clone)]
struct AppState {
    db_path: Arc<PathBuf>,
}

#[derive(Deserialize, Debug)]
pub struct ApiQueryParams {
    #[serde(default, deserialize_with = "deserialize_vec_from_str")]
    services: Vec<String>,
    #[serde(default, deserialize_with = "deserialize_vec_from_str")]
    regions: Vec<String>,
}

fn deserialize_vec_from_str<'de, D>(deserializer: D) -> std::result::Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    if s.is_empty() {
        Ok(vec![])
    } else {
        Ok(s.split(',').map(|s| s.trim().to_string()).collect())
    }
}

pub async fn start_server(db_path: PathBuf, listen_addr: String, no_browser: bool) -> Result<()> {
    let state = AppState {
        db_path: Arc::new(db_path),
    };

    let app = Router::new()
        .route("/api/query", get(query_handler))
        .nest_service("/", ServeDir::new("static"))
        .with_state(state);

    let server_url = format!("http://{}", listen_addr);
    println!("Starting server, listening on {}", server_url);

    if !no_browser {
        if let Err(e) = webbrowser::open(&server_url) {
            eprintln!("Warning: could not open browser: {}", e);
        }
    }
    let listener = tokio::net::TcpListener::bind(listen_addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

async fn query_handler(
    State(state): State<AppState>,
    Query(params): Query<ApiQueryParams>,
) -> impl IntoResponse {
    let db_path = Arc::clone(&state.db_path);
    match tokio::task::spawn_blocking(move || query::run_query(&db_path, &params.services, &params.regions)).await {
        Ok(Ok(resources)) => (StatusCode::OK, Json(resources)).into_response(),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}
