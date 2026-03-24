mod db;
mod tts;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use db::Db;
use serde::Deserialize;
use std::sync::{Arc, Mutex};
struct AppState {
    db: Mutex<Db>,
    log_path: std::path::PathBuf,
    docs_root: String,
    tts: tts::TtsManager,
}

#[derive(Deserialize)]
struct ListParams {
    status: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
}

#[derive(Deserialize)]
struct VocabListParams {
    search: Option<String>,
    reviewed: Option<bool>,
    limit: Option<i64>,
    offset: Option<i64>,
}

#[derive(Deserialize)]
struct VocabUpdateBody {
    spoken_override: Option<String>,
    reviewed: Option<bool>,
}

#[derive(Deserialize)]
struct GenerateBody {
    count: Option<usize>,
}

#[derive(Deserialize)]
struct SentenceUpdateBody {
    status: Option<String>,
    spoken: Option<String>,
}

#[derive(Deserialize)]
struct TtsPreviewBody {
    text: String,
    backend: Option<String>,
}

type AppError = (StatusCode, String);

fn err(e: impl std::fmt::Display) -> AppError {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

async fn index() -> Html<&'static str> {
    Html(include_str!("../static/index.html"))
}

// ==================== STATS ====================

async fn api_stats(State(state): State<Arc<AppState>>) -> Result<Response, AppError> {
    let db = state.db.lock().unwrap();
    let stats = db.stats().map_err(err)?;
    Ok(Json(stats).into_response())
}

// ==================== VOCAB ====================

async fn api_vocab_list(
    State(state): State<Arc<AppState>>,
    Query(params): Query<VocabListParams>,
) -> Result<Response, AppError> {
    let db = state.db.lock().unwrap();
    let list = db
        .list_vocab(
            params.search.as_deref(),
            params.reviewed,
            params.limit.unwrap_or(100),
            params.offset.unwrap_or(0),
        )
        .map_err(err)?;
    Ok(Json(list).into_response())
}

async fn api_vocab_import(State(state): State<Arc<AppState>>) -> Result<Response, AppError> {
    let docs_root = state.docs_root.clone();
    let state2 = state.clone();

    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
        let vocab = synth_textgen::corpus::extract_vocab(&docs_root)?;
        let db = state2.db.lock().unwrap();
        let count = db.import_vocab(&vocab)?;
        Ok(serde_json::json!({
            "extracted": vocab.len(),
            "imported": count,
        }))
    })
    .await
    .map_err(|e| err(e))?
    .map_err(err)?;

    Ok(Json(result).into_response())
}

async fn api_vocab_update(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(body): Json<VocabUpdateBody>,
) -> Result<Response, AppError> {
    let db = state.db.lock().unwrap();
    if let Some(ref spoken) = body.spoken_override {
        // Empty string means clear the override
        let val = if spoken.is_empty() { None } else { Some(spoken.as_str()) };
        db.update_vocab_override(id, val).map_err(err)?;
    }
    if let Some(reviewed) = body.reviewed {
        db.set_vocab_reviewed(id, reviewed).map_err(err)?;
    }
    Ok(Json(serde_json::json!({ "ok": true })).into_response())
}

// ==================== SENTENCES ====================

async fn api_sentences_list(
    State(state): State<Arc<AppState>>,
    Query(params): Query<ListParams>,
) -> Result<Response, AppError> {
    let db = state.db.lock().unwrap();
    let list = db
        .list_sentences(
            params.status.as_deref(),
            params.limit.unwrap_or(50),
            params.offset.unwrap_or(0),
        )
        .map_err(err)?;
    Ok(Json(list).into_response())
}

/// Slow: scan all sources, find sentences containing vocab terms, store as candidates
async fn api_candidates_import(
    State(state): State<Arc<AppState>>,
) -> Result<Response, AppError> {
    let docs_root = state.docs_root.clone();
    let state2 = state.clone();

    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
        let vocab = synth_textgen::corpus::extract_vocab(&docs_root)?;
        let overrides = {
            let db = state2.db.lock().unwrap();
            db.get_spoken_overrides()?
        };

        // Extract all candidate sentences from real sources
        let sentences = synth_textgen::templates::generate(
            &vocab, usize::MAX, Some(&overrides), None,
        );

        let db = state2.db.lock().unwrap();
        let mut inserted = 0usize;
        for s in &sentences {
            let vocab_json = serde_json::to_string(&s.vocab_terms)?;
            if db.insert_candidate(&s.text, &s.spoken, &vocab_json, "blog+history")? {
                inserted += 1;
            }
        }

        Ok(serde_json::json!({
            "found": sentences.len(),
            "new": inserted,
            "total": db.candidate_count()?,
        }))
    })
    .await
    .map_err(|e| err(e))?
    .map_err(err)?;

    Ok(Json(result).into_response())
}

/// Fast: pick N random candidates and promote to sentences table
async fn api_sentences_generate(
    State(state): State<Arc<AppState>>,
    Json(body): Json<GenerateBody>,
) -> Result<Response, AppError> {
    let count = body.count.unwrap_or(20) as i64;
    let db = state.db.lock().unwrap();

    let candidates = db.pick_candidates(count).map_err(err)?;
    if candidates.is_empty() {
        return Ok(Json(serde_json::json!({
            "picked": 0,
            "message": "No candidates available. Run 'Import Sources' first.",
        })).into_response());
    }

    let now = db::now_str();
    let mut inserted = 0;
    for (text, spoken, vocab_terms) in &candidates {
        db.insert_sentences(&[synth_textgen::templates::GeneratedSentence {
            text: text.clone(),
            spoken: spoken.clone(),
            vocab_terms: serde_json::from_str(vocab_terms).unwrap_or_default(),
        }]).map_err(err)?;
        inserted += 1;
    }

    Ok(Json(serde_json::json!({ "picked": inserted })).into_response())
}

async fn api_sentence_update(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(body): Json<SentenceUpdateBody>,
) -> Result<Response, AppError> {
    let db = state.db.lock().unwrap();
    if let Some(ref status) = body.status {
        db.update_sentence_status(id, status).map_err(err)?;
    }
    if let Some(ref spoken) = body.spoken {
        db.update_sentence_spoken(id, spoken).map_err(err)?;
    }
    Ok(Json(serde_json::json!({ "ok": true })).into_response())
}

// ==================== TTS PREVIEW ====================

async fn api_tts_preview(
    State(state): State<Arc<AppState>>,
    Json(body): Json<TtsPreviewBody>,
) -> Result<Response, AppError> {
    let text = body.text;
    let backend = body.backend.unwrap_or_else(|| "pocket".to_string());

    let mut audio = state.tts.generate(&backend, &text).await.map_err(err)?;
    audio.normalize();
    let wav_bytes = audio.to_wav().map_err(err)?;

    Ok((
        [(axum::http::header::CONTENT_TYPE, "audio/wav")],
        wav_bytes,
    )
        .into_response())
}

async fn api_tts_backends(
    State(state): State<Arc<AppState>>,
) -> Result<Response, AppError> {
    Ok(Json(serde_json::json!({
        "backends": state.tts.available_backends(),
    })).into_response())
}

// ==================== HARK IMPORT ====================

async fn api_hark_import(State(state): State<Arc<AppState>>) -> Result<Response, AppError> {
    let db = state.db.lock().unwrap();
    let count = db.import_hark_log(&state.log_path).map_err(err)?;
    Ok(Json(serde_json::json!({ "imported": count })).into_response())
}

// ==================== JOBS ====================

async fn api_jobs(State(state): State<Arc<AppState>>) -> Result<Response, AppError> {
    let db = state.db.lock().unwrap();
    let jobs = db.list_jobs().map_err(err)?;
    Ok(Json(jobs).into_response())
}

async fn api_job_detail(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    let db = state.db.lock().unwrap();
    match db.get_job(id).map_err(err)? {
        Some(job) => Ok(Json(job).into_response()),
        None => Ok((StatusCode::NOT_FOUND, "not found").into_response()),
    }
}

// ==================== CLI ====================

#[derive(clap::Parser)]
struct Cli {
    #[arg(long, default_value = "3456")]
    port: u16,

    #[arg(long, default_value = "corpus.db")]
    db: String,

    #[arg(long)]
    log: Option<String>,

    #[arg(long, default_value = "~/bearcove")]
    docs_root: String,

    /// Voice reference WAV for pocket-tts
    #[arg(long, default_value = "voices/amos.wav")]
    voice: String,

    /// Kokoro voice name (e.g. "af_heart", "am_adam")
    #[arg(long, default_value = "af_heart")]
    kokoro_voice: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    use clap::Parser;

    let cli = Cli::parse();

    let log_path = cli.log.map(std::path::PathBuf::from).unwrap_or_else(dirs_log_path);
    let docs_root = shellexpand::tilde(&cli.docs_root).to_string();

    // Open DB and run migrations
    let db = Db::open(std::path::Path::new(&cli.db))?;

    // Seed pronunciation overrides
    let seeded = db.seed_overrides()?;
    if seeded > 0 {
        eprintln!("Seeded {seeded} pronunciation overrides");
    }

    // Auto-import Hark log
    if log_path.exists() {
        match db.import_hark_log(&log_path) {
            Ok(n) if n > 0 => eprintln!("Imported {n} transcriptions from {}", log_path.display()),
            Ok(_) => {}
            Err(e) => eprintln!("Warning: could not import log: {e}"),
        }
    }

    // Load all available TTS backends
    let tts_manager = tts::init(&cli.voice, &cli.kokoro_voice);
    eprintln!("TTS backends: {:?}", tts_manager.available_backends());

    let state = Arc::new(AppState {
        db: Mutex::new(db),
        log_path,
        docs_root,
        tts: tts_manager,
    });

    let app = Router::new()
        // UI
        .route("/", get(index))
        // Stats
        .route("/api/stats", get(api_stats))
        // Vocab
        .route("/api/vocab", get(api_vocab_list))
        .route("/api/vocab/import", post(api_vocab_import))
        .route("/api/vocab/{id}", post(api_vocab_update))
        // Candidates + Sentences
        .route("/api/candidates/import", post(api_candidates_import))
        .route("/api/sentences", get(api_sentences_list))
        .route("/api/sentences/generate", post(api_sentences_generate))
        .route("/api/sentences/{id}", post(api_sentence_update))
        // TTS
        .route("/api/tts/backends", get(api_tts_backends))
        .route("/api/tts/preview", post(api_tts_preview))
        // Hark
        .route("/api/hark/import", post(api_hark_import))
        // Jobs
        .route("/api/jobs", get(api_jobs))
        .route("/api/jobs/{id}", get(api_job_detail))
        .with_state(state);

    let addr = format!("127.0.0.1:{}", cli.port);
    eprintln!("Corpus dashboard listening on http://{addr}");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

fn dirs_log_path() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    std::path::PathBuf::from(home).join("Library/Application Support/hark/transcription_log.jsonl")
}
