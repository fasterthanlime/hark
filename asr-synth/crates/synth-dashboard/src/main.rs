mod db;
mod jobs;
mod review;
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

pub type AppError = (StatusCode, String);

pub struct AppState {
    db: Mutex<Db>,
    log_path: std::path::PathBuf,
    docs_root: String,
    tts: tts::TtsManager,
    asr: qwen3_asr::AsrInference,
    parakeet: std::sync::Mutex<parakeet_rs::ParakeetTDT>,
    aligner: qwen3_asr::ForcedAligner,
    review: Mutex<review::ReviewSession>,
    precompute_notify: std::sync::Arc<tokio::sync::Notify>,
    audio_dir: String,
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
    has_override: Option<bool>,
    limit: Option<i64>,
    offset: Option<i64>,
}

#[derive(Deserialize)]
struct VocabUpdateBody {
    spoken_override: Option<String>,
    reviewed: Option<bool>,
}

#[derive(Deserialize)]
struct VocabAddBody {
    term: String,
    spoken_override: Option<String>,
}

#[derive(Deserialize)]
struct GenerateBody {
    count: Option<usize>,
    prioritize_unknown: Option<bool>,
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
    /// When set, also run forced alignment on the generated audio with this text
    /// and return JSON instead of raw WAV.
    align_text: Option<String>,
}

pub fn err(e: impl std::fmt::Display) -> AppError {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

async fn index() -> Result<Html<String>, AppError> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("static/index.html");
    let content = std::fs::read_to_string(&path).map_err(err)?;
    Ok(Html(content))
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
            params.has_override,
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

async fn api_vocab_add(
    State(state): State<Arc<AppState>>,
    Json(body): Json<VocabAddBody>,
) -> Result<Response, AppError> {
    let term = body.term.trim().to_string();
    if term.is_empty() {
        return Ok(Json(serde_json::json!({"error": "term is empty"})).into_response());
    }
    let spoken_auto = synth_textgen::corpus::to_spoken(&term);
    let db = state.db.lock().unwrap();
    db.insert_candidate_vocab(&term, &spoken_auto).map_err(err)?;
    if let Some(ref spoken) = body.spoken_override {
        if !spoken.is_empty() {
            if let Ok(Some(row)) = db.find_vocab_by_term(&term) {
                db.update_vocab_override(row.id, Some(spoken)).map_err(err)?;
            }
        }
    }
    Ok(Json(serde_json::json!({"ok": true})).into_response())
}

// ==================== SENTENCES ====================

async fn api_sentences_list(
    State(state): State<Arc<AppState>>,
    Query(params): Query<ListParams>,
) -> Result<Response, AppError> {
    let db = state.db.lock().unwrap();
    let limit = params.limit.unwrap_or(50);

    // When requesting pending sentences, auto-promote candidates if we're running low
    if params.status.as_deref() == Some("pending") {
        let pending_count = db.list_sentences(Some("pending"), 1, 0).map_err(err)?.len() as i64;
        if pending_count < limit {
            let needed = limit - pending_count;
            let candidates = db.pick_candidates(needed, true).map_err(err)?;
            for (text, spoken, vocab_terms, unknown_words) in &candidates {
                let _ = db.insert_sentence_from_candidate(text, spoken, vocab_terms, unknown_words);
            }
        }
    }

    let list = db
        .list_sentences(
            params.status.as_deref(),
            limit,
            params.offset.unwrap_or(0),
        )
        .map_err(err)?;
    Ok(Json(list).into_response())
}

/// Scan sources, extract vocab, find sentences, detect unknown words, store directly as sentences.
/// Single pass — no intermediate candidates step.
async fn api_candidates_import(
    State(state): State<Arc<AppState>>,
) -> Result<Response, AppError> {
    let docs_root = state.docs_root.clone();
    let state2 = state.clone();

    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
        eprintln!("[import] extracting vocab from {docs_root}...");
        let vocab = synth_textgen::corpus::extract_vocab(&docs_root)?;
        eprintln!("[import] {} vocab terms extracted", vocab.len());

        let overrides = {
            let db = state2.db.lock().unwrap();
            db.get_spoken_overrides()?
        };

        eprintln!("[import] scanning history for sentences...");
        let sentences = synth_textgen::templates::generate(
            &vocab, usize::MAX, Some(&overrides), None,
        );
        eprintln!("[import] {} sentences found", sentences.len());

        let db = state2.db.lock().unwrap();
        let mut inserted = 0usize;
        let mut new_vocab = 0usize;
        let total = sentences.len();
        for (i, s) in sentences.iter().enumerate() {
            if i > 0 && i % 500 == 0 {
                eprintln!("[import] {i}/{total} ({inserted} new)");
            }
            let unknown = tts::detect_unknown_words(&s.text);
            let unknown_json = serde_json::to_string(&unknown)?;

            // Insert unknown words as vocab entries
            for w in &unknown {
                if db.insert_candidate_vocab(w, &w.to_lowercase())? {
                    new_vocab += 1;
                }
            }

            // Insert directly as a sentence ready for review
            let vocab_json = serde_json::to_string(&s.vocab_terms)?;
            // Insert directly as sentence (skips duplicates)
            if db.insert_sentence_from_candidate(&s.text, &s.spoken, &vocab_json, &unknown_json)? {
                inserted += 1;
            }
        }

        eprintln!("[import] done. {inserted} sentences, {new_vocab} new vocab entries");

        Ok(serde_json::json!({
            "imported": inserted,
            "total_sentences": total,
            "new_vocab": new_vocab,
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
    let prioritize = body.prioritize_unknown.unwrap_or(true);
    let db = state.db.lock().unwrap();

    let candidates = db.pick_candidates(count, prioritize).map_err(err)?;
    if candidates.is_empty() {
        return Ok(Json(serde_json::json!({
            "picked": 0,
            "message": "No candidates available. Run 'Import Sources' first.",
        })).into_response());
    }

    let mut inserted = 0;
    for (text, spoken, vocab_terms, unknown_words) in &candidates {
        db.insert_sentence_from_candidate(text, spoken, vocab_terms, unknown_words).map_err(err)?;
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
    let backend = body.backend.unwrap_or_else(|| "pocket-hq".to_string());
    let align_text = body.align_text;

    eprintln!("TTS preview: backend={backend} text={:?}", &text[..text.len().min(50)]);
    let mut audio = state.tts.generate(&backend, &text).await.map_err(|e| {
        eprintln!("TTS error: {e}");
        err(e)
    })?;
    audio.normalize();
    let wav_bytes = audio.to_wav().map_err(err)?;

    // If align_text provided, run forced alignment and return JSON
    if let Some(align_text) = align_text {
        let samples = audio.samples.clone();
        let sample_rate = audio.sample_rate;
        let state2 = state.clone();

        let alignment = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<serde_json::Value>> {
            // Resample to 16kHz for the aligner
            let samples_16k = if sample_rate != 16000 {
                use rubato::{Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction};
                let params = SincInterpolationParameters {
                    sinc_len: 256,
                    f_cutoff: 0.95,
                    interpolation: SincInterpolationType::Linear,
                    oversampling_factor: 256,
                    window: WindowFunction::BlackmanHarris2,
                };
                let mut resampler = SincFixedIn::<f32>::new(
                    16000.0 / sample_rate as f64, 2.0, params, samples.len(), 1,
                )?;
                let output = resampler.process(&[&samples], None)?;
                output.into_iter().next().unwrap_or_default()
            } else {
                samples
            };

            let items = state2.aligner.align(&samples_16k, &align_text)
                .map_err(|e| anyhow::anyhow!("Aligner: {e}"))?;

            Ok(items.iter().map(|item| serde_json::json!({
                "word": item.word,
                "start": item.start_time,
                "end": item.end_time,
            })).collect())
        })
        .await
        .map_err(|e| err(e))?
        .map_err(err)?;

        use base64::Engine;
        let audio_b64 = base64::engine::general_purpose::STANDARD.encode(&wav_bytes);

        return Ok(Json(serde_json::json!({
            "audio_b64": audio_b64,
            "alignment": alignment,
        })).into_response());
    }

    Ok((
        [(axum::http::header::CONTENT_TYPE, "audio/wav")],
        wav_bytes,
    )
        .into_response())
}

// ==================== G2P SCAN ====================

#[derive(Deserialize)]
struct G2pScanBody {
    text: String,
}

async fn api_g2p_scan(
    Json(body): Json<G2pScanBody>,
) -> Result<Response, AppError> {
    let text = body.text;
    let unknown = tokio::task::spawn_blocking(move || tts::detect_unknown_words(&text))
        .await
        .map_err(|e| err(e))?;
    Ok(Json(serde_json::json!({ "unknown_words": unknown })).into_response())
}

async fn api_tts_backends(
    State(state): State<Arc<AppState>>,
) -> Result<Response, AppError> {
    Ok(Json(serde_json::json!({
        "backends": state.tts.available_backends(),
    })).into_response())
}

// ==================== ASR ====================

async fn api_asr_transcribe(
    State(state): State<Arc<AppState>>,
    body: axum::body::Bytes,
) -> Result<Response, AppError> {
    // body is raw WAV bytes — decode to f32 samples, resample to 16kHz
    let state2 = state.clone();
    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<String> {
        let cursor = std::io::Cursor::new(body.to_vec());
        let mut reader = hound::WavReader::new(cursor)
            .map_err(|e| anyhow::anyhow!("WAV decode: {e}"))?;
        let spec = reader.spec();

        // Convert to f32 samples
        let samples_f32: Vec<f32> = match spec.sample_format {
            hound::SampleFormat::Float => {
                reader.samples::<f32>().filter_map(|s| s.ok()).collect()
            }
            hound::SampleFormat::Int => {
                let max = (1i64 << (spec.bits_per_sample - 1)) as f32;
                reader.samples::<i32>().filter_map(|s| s.ok()).map(|s| s as f32 / max).collect()
            }
        };

        // Convert to mono if stereo
        let mono = if spec.channels > 1 {
            samples_f32.chunks(spec.channels as usize)
                .map(|ch| ch.iter().sum::<f32>() / ch.len() as f32)
                .collect()
        } else {
            samples_f32
        };

        // Resample to 16kHz if needed
        let samples_16k = if spec.sample_rate != 16000 {
            use rubato::{Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction};
            let params = SincInterpolationParameters {
                sinc_len: 256,
                f_cutoff: 0.95,
                interpolation: SincInterpolationType::Linear,
                oversampling_factor: 256,
                window: WindowFunction::BlackmanHarris2,
            };
            let mut resampler = SincFixedIn::<f32>::new(
                16000.0 / spec.sample_rate as f64, 2.0, params, mono.len(), 1,
            )?;
            let output = resampler.process(&[&mono], None)?;
            output.into_iter().next().unwrap_or_default()
        } else {
            mono
        };

        let result = state2.asr.transcribe_samples(
            &samples_16k,
            qwen3_asr::TranscribeOptions::default(),
        ).map_err(|e| anyhow::anyhow!("ASR: {e}"))?;

        Ok(result.text)
    })
    .await
    .map_err(|e| err(e))?
    .map_err(err)?;

    Ok(Json(serde_json::json!({ "text": result })).into_response())
}

// ==================== FORCED ALIGNMENT ====================

async fn api_align(
    State(state): State<Arc<AppState>>,
    mut multipart: axum::extract::Multipart,
) -> Result<Response, AppError> {
    let mut audio_bytes: Option<Vec<u8>> = None;
    let mut text: Option<String> = None;

    while let Some(field) = multipart.next_field().await.map_err(|e| err(e))? {
        match field.name() {
            Some("audio") => {
                audio_bytes = Some(field.bytes().await.map_err(|e| err(e))?.to_vec());
            }
            Some("text") => {
                text = Some(field.text().await.map_err(|e| err(e))?);
            }
            _ => {}
        }
    }

    let audio_bytes = audio_bytes.ok_or_else(|| err(anyhow::anyhow!("missing 'audio' field")))?;
    let text = text.ok_or_else(|| err(anyhow::anyhow!("missing 'text' field")))?;

    let state2 = state.clone();
    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<qwen3_asr::ForcedAlignItem>> {
        // Decode WAV
        let cursor = std::io::Cursor::new(audio_bytes);
        let mut reader = hound::WavReader::new(cursor)
            .map_err(|e| anyhow::anyhow!("WAV decode: {e}"))?;
        let spec = reader.spec();

        let samples_f32: Vec<f32> = match spec.sample_format {
            hound::SampleFormat::Float => {
                reader.samples::<f32>().filter_map(|s| s.ok()).collect()
            }
            hound::SampleFormat::Int => {
                let max = (1i64 << (spec.bits_per_sample - 1)) as f32;
                reader.samples::<i32>().filter_map(|s| s.ok()).map(|s| s as f32 / max).collect()
            }
        };

        let mono = if spec.channels > 1 {
            samples_f32.chunks(spec.channels as usize)
                .map(|ch| ch.iter().sum::<f32>() / ch.len() as f32)
                .collect()
        } else {
            samples_f32
        };

        // Resample to 16kHz
        let samples_16k = if spec.sample_rate != 16000 {
            use rubato::{Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction};
            let params = SincInterpolationParameters {
                sinc_len: 256,
                f_cutoff: 0.95,
                interpolation: SincInterpolationType::Linear,
                oversampling_factor: 256,
                window: WindowFunction::BlackmanHarris2,
            };
            let mut resampler = SincFixedIn::<f32>::new(
                16000.0 / spec.sample_rate as f64, 2.0, params, mono.len(), 1,
            )?;
            let output = resampler.process(&[&mono], None)?;
            output.into_iter().next().unwrap_or_default()
        } else {
            mono
        };

        state2.aligner.align(&samples_16k, &text)
            .map_err(|e| anyhow::anyhow!("Aligner: {e}"))
    })
    .await
    .map_err(|e| err(e))?
    .map_err(err)?;

    let alignment: Vec<serde_json::Value> = result.iter().map(|item| {
        serde_json::json!({
            "word": item.word,
            "start": item.start_time,
            "end": item.end_time,
        })
    }).collect();

    Ok(Json(serde_json::json!({ "alignment": alignment })).into_response())
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

    /// Kokoro voice name (e.g. "am_puck", "am_adam", "af_heart")
    #[arg(long, default_value = "am_puck")]
    kokoro_voice: String,

    /// Qwen3 ASR model directory (GGUF quantized)
    #[arg(long, default_value = "~/Library/Caches/qwen3-asr/Alkd--qwen3-asr-gguf--qwen3_asr_1_7b_q8_0_gguf")]
    qwen_model: String,

    /// Parakeet TDT model directory
    #[arg(long, default_value = "models/parakeet-tdt")]
    parakeet_model: String,

    /// Qwen3 ForcedAligner model ID (downloaded from HuggingFace Hub)
    #[arg(long, default_value = "Qwen/Qwen3-ForcedAligner-0.6B")]
    aligner_model: String,

    /// Cache directory for HuggingFace Hub downloads
    #[arg(long, default_value = "~/Library/Caches/qwen3-asr")]
    hf_cache: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    use clap::Parser;

    let cli = Cli::parse();

    let log_path = cli.log.map(std::path::PathBuf::from).unwrap_or_else(dirs_log_path);
    let docs_root = shellexpand::tilde(&cli.docs_root).to_string();

    // Load CMUdict for unknown word detection
    tts::init_cmudict();

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

    // Load Parakeet TDT
    eprintln!("Loading Parakeet TDT...");
    let parakeet = parakeet_rs::ParakeetTDT::from_pretrained(&cli.parakeet_model, None)?;
    eprintln!("Parakeet ready");

    // Load Qwen3 ASR model
    let qwen_model_dir = shellexpand::tilde(&cli.qwen_model).to_string();
    eprintln!("Loading Qwen3 ASR from {qwen_model_dir}...");
    let asr = qwen3_asr::AsrInference::load(
        std::path::Path::new(&qwen_model_dir),
        qwen3_asr::best_device(),
    )?;
    eprintln!("Qwen3 ASR ready");

    // Load ForcedAligner (downloads from HF Hub if needed)
    let hf_cache = shellexpand::tilde(&cli.hf_cache).to_string();
    eprintln!("Loading ForcedAligner ({})...", cli.aligner_model);
    let aligner = qwen3_asr::ForcedAligner::from_pretrained(
        &cli.aligner_model,
        std::path::Path::new(&hf_cache),
        qwen3_asr::best_device(),
    )?;
    eprintln!("ForcedAligner ready");

    let precompute_notify = std::sync::Arc::new(tokio::sync::Notify::new());
    let audio_dir = "audio".to_string();
    std::fs::create_dir_all(&audio_dir).ok();

    let state = Arc::new(AppState {
        db: Mutex::new(db),
        log_path,
        docs_root,
        tts: tts_manager,
        asr,
        parakeet: std::sync::Mutex::new(parakeet),
        aligner,
        review: Mutex::new(review::ReviewSession::new()),
        precompute_notify: precompute_notify.clone(),
        audio_dir: audio_dir.clone(),
    });

    // Start background pre-computation loop
    review::spawn_precompute_loop(state.clone(), precompute_notify, audio_dir);

    let app = Router::new()
        // UI
        .route("/", get(index))
        // Stats
        .route("/api/stats", get(api_stats))
        // Vocab
        .route("/api/vocab", get(api_vocab_list).post(api_vocab_add))
        .route("/api/vocab/import", post(api_vocab_import))
        .route("/api/vocab/{id}", post(api_vocab_update))
        // Candidates + Sentences
        .route("/api/candidates/import", post(api_candidates_import))
        .route("/api/sentences", get(api_sentences_list))
        .route("/api/sentences/generate", post(api_sentences_generate))
        .route("/api/sentences/{id}", post(api_sentence_update))
        // TTS + G2P
        .route("/api/tts/backends", get(api_tts_backends))
        .route("/api/tts/preview", post(api_tts_preview))
        .route("/api/g2p/scan", post(api_g2p_scan))
        // ASR
        .route("/api/asr/transcribe", post(api_asr_transcribe))
        // Forced alignment
        .route("/api/align", post(api_align))
        // Review (server-side orchestrated)
        .route("/api/review/current", get(review::api_review_current))
        .route("/api/review/current/approve", post(review::api_review_approve))
        .route("/api/review/current/reject", post(review::api_review_reject))
        .route("/api/review/current/pronunciation", post(review::api_review_pronunciation))
        .route("/api/review/current/text", post(review::api_review_edit_text))
        .route("/api/review/current/backend", post(review::api_review_backend))
        .route("/api/review/current/asr", post(review::api_review_asr))
        // Hark
        .route("/api/hark/import", post(api_hark_import))
        // Jobs
        .route("/api/jobs", get(api_jobs))
        .route("/api/jobs/{id}", get(api_job_detail))
        // Pipeline jobs
        .route("/api/jobs/corpus", post(jobs::api_start_corpus_job))
        .route("/api/jobs/prepare", post(jobs::api_start_prepare_job))
        .route("/api/jobs/train", post(jobs::api_start_train_job))
        .route("/api/pipeline/status", get(jobs::api_pipeline_status))
        .with_state(state);

    let addr = format!("127.0.0.1:{}", cli.port);
    eprintln!("hark ml listening on http://{addr}");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

fn dirs_log_path() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    std::path::PathBuf::from(home).join("Library/Application Support/hark/transcription_log.jsonl")
}
