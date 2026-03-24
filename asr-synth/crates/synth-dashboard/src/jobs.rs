use std::io::Write;
use std::sync::Arc;

use axum::{
    extract::State,
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;

use crate::tts;
use crate::{err, AppError, AppState};
use parakeet_rs::Transcriber;

// ==================== Job Configs ====================

#[derive(Deserialize)]
pub struct CorpusJobBody {
    pub tts_backend: Option<String>,
}

#[derive(Deserialize)]
pub struct PrepareJobBody {
    pub identity_count: Option<usize>,
}

#[derive(Deserialize)]
pub struct TrainJobBody {
    pub model: Option<String>,
    pub iters: Option<usize>,
    pub batch_size: Option<usize>,
    pub num_layers: Option<usize>,
}

// ==================== Job Guard ====================

fn check_no_running_jobs(state: &Arc<AppState>) -> Result<(), AppError> {
    let db = state.db.lock().unwrap();
    let jobs = db.list_jobs().map_err(err)?;
    for job in &jobs {
        if job.status == "running" {
            return Err(err(anyhow::anyhow!(
                "Cannot start job: job #{} ({}) is still running",
                job.id,
                job.job_type
            )));
        }
    }
    Ok(())
}

// ==================== Corpus Generation ====================

pub async fn api_start_corpus_job(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CorpusJobBody>,
) -> Result<Response, AppError> {
    check_no_running_jobs(&state)?;

    let tts_backend = body.tts_backend.unwrap_or_else(|| "openai".to_string());
    let config_json = serde_json::json!({"tts_backend": tts_backend}).to_string();

    let job_id = {
        let db = state.db.lock().unwrap();
        db.create_job("corpus", Some(&config_json)).map_err(err)?
    };

    let state2 = state.clone();
    let backend = tts_backend.clone();
    tokio::spawn(async move {
        let result = run_corpus_job(&state2, job_id, &backend).await;
        let db = state2.db.lock().unwrap();
        match result {
            Ok(count) => {
                let _ = db.finish_job(
                    job_id,
                    "completed",
                    Some(&serde_json::json!({"sentences": count}).to_string()),
                );
            }
            Err(e) => {
                let _ = db.append_job_log(job_id, &format!("ERROR: {e}"));
                let _ = db.finish_job(job_id, "failed", None);
            }
        }
    });

    Ok(Json(serde_json::json!({"job_id": job_id})).into_response())
}

async fn run_corpus_job(
    state: &Arc<AppState>,
    job_id: i64,
    tts_backend: &str,
) -> anyhow::Result<usize> {
    let sentences = {
        let db = state.db.lock().unwrap();
        db.list_approved_sentences()?
    };
    let total = sentences.len();

    {
        let db = state.db.lock().unwrap();
        db.append_job_log(job_id, &format!("Starting corpus generation: {total} approved sentences, backend: {tts_backend}"))?;
    }

    // Ensure data directory exists
    std::fs::create_dir_all("data").ok();
    let mut file = std::io::BufWriter::new(
        std::fs::File::create("data/corpus_dashboard.jsonl")
            .map_err(|e| anyhow::anyhow!("Failed to create corpus file: {e}"))?,
    );

    let mut count = 0;
    for (i, sentence) in sentences.iter().enumerate() {
        let text_preview: String = sentence.text.chars().take(50).collect();

        // TTS (async for remote backends)
        let audio = match state.tts.generate(tts_backend, &sentence.spoken).await {
            Ok(mut a) => {
                a.normalize();
                a
            }
            Err(e) => {
                let db = state.db.lock().unwrap();
                let _ = db.append_job_log(job_id, &format!("[{}/{}] TTS FAILED: {e} — {text_preview}", i + 1, total));
                continue;
            }
        };

        // Resample to 16kHz for ASR
        let samples_16k = match tts::resample_to_16k(&audio.samples, audio.sample_rate) {
            Ok(s) => s,
            Err(e) => {
                let db = state.db.lock().unwrap();
                let _ = db.append_job_log(job_id, &format!("[{}/{}] Resample FAILED: {e} — {text_preview}", i + 1, total));
                continue;
            }
        };

        // ASR (blocking)
        let state2 = state.clone();
        let samples_clone = samples_16k.clone();
        let asr_result = tokio::task::spawn_blocking(move || -> anyhow::Result<(String, String)> {
            let qwen = state2
                .asr
                .transcribe_samples(&samples_clone, qwen3_asr::TranscribeOptions::default())
                .map(|r| r.text)
                .unwrap_or_default();

            let parakeet = {
                let mut p = state2.parakeet.lock().unwrap();
                p.transcribe_samples(samples_clone.to_vec(), 16000, 1, None)
                    .map(|r| r.text)
                    .unwrap_or_default()
            };

            Ok((qwen, parakeet))
        })
        .await??;

        let (qwen, parakeet) = asr_result;

        // Write JSONL line
        writeln!(
            file,
            "{}",
            serde_json::json!({
                "original": sentence.text,
                "parakeet": parakeet,
                "qwen": qwen,
            })
        )?;
        count += 1;

        // Log progress
        {
            let db = state.db.lock().unwrap();
            let _ = db.append_job_log(
                job_id,
                &format!("[{}/{}] {text_preview}...", i + 1, total),
            );
        }
    }

    file.flush()?;

    {
        let db = state.db.lock().unwrap();
        db.append_job_log(job_id, &format!("Done: {count}/{total} sentences written to data/corpus_dashboard.jsonl"))?;
    }

    Ok(count)
}

// ==================== Prepare ====================

pub async fn api_start_prepare_job(
    State(state): State<Arc<AppState>>,
    Json(body): Json<PrepareJobBody>,
) -> Result<Response, AppError> {
    check_no_running_jobs(&state)?;

    let identity_count = body.identity_count.unwrap_or(95000);
    let config_json = serde_json::json!({"identity_count": identity_count}).to_string();

    let job_id = {
        let db = state.db.lock().unwrap();
        db.create_job("prepare", Some(&config_json)).map_err(err)?
    };

    let state2 = state.clone();
    tokio::task::spawn_blocking(move || {
        let config = synth_train::PrepareConfig {
            input: "data/corpus_dashboard.jsonl".into(),
            identity_count,
            ..Default::default()
        };

        let result = synth_train::prepare(&config, |msg| {
            let db = state2.db.lock().unwrap();
            let _ = db.append_job_log(job_id, msg);
        });

        let db = state2.db.lock().unwrap();
        match result {
            Ok(stats) => {
                let _ = db.append_job_log(
                    job_id,
                    &format!(
                        "Done: {} corrections + {} identity = {} train / {} valid / {} test",
                        stats.correction_examples,
                        stats.identity_examples,
                        stats.train_count,
                        stats.valid_count,
                        stats.test_count,
                    ),
                );
                let _ = db.finish_job(
                    job_id,
                    "completed",
                    Some(
                        &serde_json::json!({
                            "correction_examples": stats.correction_examples,
                            "identity_examples": stats.identity_examples,
                            "train_count": stats.train_count,
                            "valid_count": stats.valid_count,
                            "test_count": stats.test_count,
                        })
                        .to_string(),
                    ),
                );
            }
            Err(e) => {
                let _ = db.append_job_log(job_id, &format!("ERROR: {e}"));
                let _ = db.finish_job(job_id, "failed", None);
            }
        }
    });

    Ok(Json(serde_json::json!({"job_id": job_id})).into_response())
}

// ==================== Train ====================

pub async fn api_start_train_job(
    State(state): State<Arc<AppState>>,
    Json(body): Json<TrainJobBody>,
) -> Result<Response, AppError> {
    check_no_running_jobs(&state)?;

    let config = synth_train::TrainConfig {
        model: body.model.unwrap_or_else(|| "Qwen/Qwen2.5-0.5B".into()),
        iters: body.iters.unwrap_or(1000),
        batch_size: body.batch_size.unwrap_or(1),
        num_layers: body.num_layers.unwrap_or(4),
        ..Default::default()
    };
    let config_json = serde_json::json!({
        "model": config.model,
        "iters": config.iters,
        "batch_size": config.batch_size,
        "num_layers": config.num_layers,
    })
    .to_string();

    let job_id = {
        let db = state.db.lock().unwrap();
        db.create_job("train", Some(&config_json)).map_err(err)?
    };

    let state2 = state.clone();
    tokio::task::spawn_blocking(move || {
        {
            let db = state2.db.lock().unwrap();
            let _ = db.append_job_log(
                job_id,
                &format!(
                    "Starting training: model={}, iters={}, batch_size={}, num_layers={}",
                    config.model, config.iters, config.batch_size, config.num_layers
                ),
            );
        }

        let result = synth_train::train_streaming(&config, |line| {
            let db = state2.db.lock().unwrap();
            let _ = db.append_job_log(job_id, line);
        });

        let db = state2.db.lock().unwrap();
        match result {
            Ok(status) if status.success() => {
                let _ = db.append_job_log(job_id, "Training completed successfully");
                let _ = db.finish_job(job_id, "completed", Some(&serde_json::json!({"exit_code": 0}).to_string()));
            }
            Ok(status) => {
                let code = status.code().unwrap_or(-1);
                let _ = db.append_job_log(job_id, &format!("Training exited with code {code}"));
                let _ = db.finish_job(job_id, "failed", Some(&serde_json::json!({"exit_code": code}).to_string()));
            }
            Err(e) => {
                let _ = db.append_job_log(job_id, &format!("ERROR: {e}"));
                let _ = db.finish_job(job_id, "failed", None);
            }
        }
    });

    Ok(Json(serde_json::json!({"job_id": job_id})).into_response())
}

// ==================== Pipeline Status ====================

pub async fn api_pipeline_status(
    State(state): State<Arc<AppState>>,
) -> Result<Response, AppError> {
    let (approved_count, human_recordings, running_job) = {
        let db = state.db.lock().unwrap();
        let (approved, _, _) = db.sentence_count_by_status().map_err(err)?;
        let human = db.sentences_with_human_recording_count().map_err(err)?;
        let jobs = db.list_jobs().map_err(err)?;
        let running = jobs.into_iter().find(|j| j.status == "running");
        (approved, human, running)
    };

    // Check filesystem for corpus / training data / adapters
    let corpus_exists = std::path::Path::new("data/corpus_dashboard.jsonl").exists();
    let corpus_lines = if corpus_exists {
        std::fs::read_to_string("data/corpus_dashboard.jsonl")
            .map(|s| s.lines().filter(|l| !l.trim().is_empty()).count())
            .unwrap_or(0)
    } else {
        0
    };

    let training_data_exists = std::path::Path::new("training/data/train.jsonl").exists();
    let train_count = if training_data_exists {
        std::fs::read_to_string("training/data/train.jsonl")
            .map(|s| s.lines().filter(|l| !l.trim().is_empty()).count())
            .unwrap_or(0)
    } else {
        0
    };

    let adapters_exist = std::path::Path::new("training/adapters").exists()
        && std::fs::read_dir("training/adapters")
            .map(|mut d| d.next().is_some())
            .unwrap_or(false);

    let backends = state.tts.available_backends();

    let running_json = running_job.map(|j| {
        serde_json::json!({
            "id": j.id,
            "job_type": j.job_type,
            "status": j.status,
        })
    });

    Ok(Json(serde_json::json!({
        "approved_count": approved_count,
        "corpus_exists": corpus_exists,
        "corpus_lines": corpus_lines,
        "training_data_exists": training_data_exists,
        "train_count": train_count,
        "adapters_exist": adapters_exist,
        "human_recordings": human_recordings,
        "backends": backends,
        "running_job": running_json,
    }))
    .into_response())
}
