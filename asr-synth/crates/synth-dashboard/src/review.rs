use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use axum::{
    extract::State,
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use tokio::sync::Notify;

use crate::db::{Db, SentenceRow};
use parakeet_rs::Transcriber;
use crate::tts;
use crate::{AppState, err, AppError};

/// Clean a word for comparison: strip non-alphanumeric, lowercase.
fn clean_word(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase()
}

/// Align original words to aligner output using greedy concatenation.
///
/// The aligner may:
/// - Drop words entirely (punctuation, short words)
/// - Split one word into multiple tokens ("HIR" → "H", "I", "R")
/// - Transform words ("don't" → "dont")
///
/// Strategy: walk original words. For each, try to match by consuming one or more
/// consecutive aligner tokens whose concatenation equals the cleaned original word.
/// If no match, the word was dropped — fill from nearest neighbor.
fn match_alignment(
    original_words: &[&str],
    aligner_items: &[qwen3_asr::ForcedAlignItem],
) -> Vec<serde_json::Value> {
    let mut aligned: Vec<(usize, f64, f64)> = Vec::new(); // (orig_idx, start, end)
    let mut ai = 0; // current position in aligner items

    for (oi, orig_word) in original_words.iter().enumerate() {
        let orig_clean = clean_word(orig_word);
        if orig_clean.is_empty() {
            // Punctuation-only token — will be filled from neighbor
            continue;
        }

        // Try consuming 1..=N aligner tokens starting at ai
        let mut matched = false;
        'outer: for take in 1..=5.min(aligner_items.len().saturating_sub(ai)) {
            let mut concat = String::new();
            for k in 0..take {
                concat.push_str(&clean_word(&aligner_items[ai + k].word));
            }
            if concat == orig_clean {
                // Match! Use the time range spanning all consumed tokens
                let start = aligner_items[ai].start_time;
                let end = aligner_items[ai + take - 1].end_time;
                aligned.push((oi, start, end));
                ai += take;
                matched = true;
                break 'outer;
            }
        }

        if !matched {
            // Maybe the aligner skipped ahead — peek if a later aligner token matches
            // (handles aligner inserting extra tokens before this word)
            for skip in 1..=3.min(aligner_items.len().saturating_sub(ai)) {
                for take in 1..=3.min(aligner_items.len().saturating_sub(ai + skip)) {
                    let mut concat = String::new();
                    for k in 0..take {
                        concat.push_str(&clean_word(&aligner_items[ai + skip + k].word));
                    }
                    if concat == orig_clean {
                        let start = aligner_items[ai + skip].start_time;
                        let end = aligner_items[ai + skip + take - 1].end_time;
                        aligned.push((oi, start, end));
                        ai = ai + skip + take;
                        matched = true;
                        break;
                    }
                }
                if matched { break; }
            }
        }
        // If still not matched, this word is "dropped" — no entry in aligned
    }

    // Build the full result, filling in dropped words from nearest neighbor
    let mut result: Vec<serde_json::Value> = Vec::with_capacity(original_words.len());
    let mut next_aligned = 0;

    for (oi, orig_word) in original_words.iter().enumerate() {
        if next_aligned < aligned.len() && aligned[next_aligned].0 == oi {
            let (_, start, end) = aligned[next_aligned];
            result.push(serde_json::json!({"word": orig_word, "start": start, "end": end}));
            next_aligned += 1;
        } else {
            // Dropped — inherit time from nearest aligned neighbor
            let time = if next_aligned > 0 {
                let prev = &aligned[next_aligned - 1];
                (prev.1, prev.2)
            } else if next_aligned < aligned.len() {
                let nxt = &aligned[next_aligned];
                (nxt.1, nxt.2)
            } else {
                (0.0, 0.0)
            };
            result.push(serde_json::json!({"word": orig_word, "start": time.0, "end": time.1}));
        }
    }
    result
}

// ==================== Review Session State ====================

pub struct ReviewSession {
    pub current_id: Option<i64>,
    pub queue: VecDeque<i64>,
    pub backend: String,
    pub precomputed: HashMap<i64, PrecomputedData>,
}

pub struct PrecomputedData {
    pub audio_b64: String,
    pub alignment: Vec<serde_json::Value>,         // spoken text alignment (for waveform)
    pub written_alignment: Vec<serde_json::Value>,  // written text alignment (for transcript grid)
    pub qwen_alignment: Vec<serde_json::Value>,
    pub parakeet_alignment: Vec<serde_json::Value>,
    pub wav_path: String,
    pub qwen_asr: String,
    pub parakeet_asr: String,
}

impl ReviewSession {
    pub fn new() -> Self {
        Self {
            current_id: None,
            queue: VecDeque::new(),
            backend: "pocket-hq".to_string(),
            precomputed: HashMap::new(),
        }
    }
}

// ==================== Compute Review Data ====================

/// Compute TTS + alignment for a sentence. Blocking — call from spawn_blocking.
pub fn compute_for_sentence(
    state: &Arc<AppState>,
    sentence: &SentenceRow,
    backend: &str,
    audio_dir: &str,
) -> anyhow::Result<PrecomputedData> {
    // Generate TTS — replace hyphens with spaces for better pronunciation
    let spoken_owned = sentence.spoken.replace('-', " ");
    let spoken_text = if sentence.spoken != sentence.text {
        &spoken_owned
    } else {
        // Even if spoken == text, normalize hyphens for TTS
        &spoken_owned
    };

    // TTS is async for remote backends — we need a runtime handle
    let rt = tokio::runtime::Handle::current();
    let mut audio = rt.block_on(state.tts.generate(backend, spoken_text))?;
    audio.normalize();
    let wav_bytes = audio.to_wav()?;

    // Save WAV to disk
    std::fs::create_dir_all(audio_dir).ok();
    let wav_path = format!("{}/{}.wav", audio_dir, sentence.id);
    std::fs::write(&wav_path, &wav_bytes)?;

    // Resample to 16kHz for aligner
    let samples_16k = tts::resample_to_16k(&audio.samples, audio.sample_rate)?;

    // Run forced alignment on spoken text (for waveform playback sync)
    let spoken_items = state.aligner.align(&samples_16k, spoken_text)
        .map_err(|e| anyhow::anyhow!("Aligner (spoken): {e}"))?;

    // alignment is built below after written_alignment, filling in dropped words

    // Run forced alignment on written text (for transcript grid display row)
    let original_words: Vec<&str> = sentence.text.split_whitespace().collect();
    let written_items = state.aligner.align(&samples_16k, &sentence.text)
        .unwrap_or_default();

    // Map aligner words back to original words, handling splits/drops/transforms
    let written_alignment = match_alignment(&original_words, &written_items);

    // Do the same for spoken alignment — fill in dropped words
    let spoken_words: Vec<&str> = spoken_text.split_whitespace().collect();
    let alignment = match_alignment(&spoken_words, &spoken_items);

    // Run ASR on the TTS audio (round-trip quality check)
    let qwen_asr = match state.asr.transcribe_samples(&samples_16k, qwen3_asr::TranscribeOptions::default()) {
        Ok(r) => r.text,
        Err(e) => { eprintln!("[review] Qwen ASR failed: {e}"); String::new() }
    };

    let parakeet_asr = {
        let mut parakeet = state.parakeet.lock().unwrap();
        match parakeet.transcribe_samples(samples_16k.to_vec(), 16000, 1, None) {
            Ok(r) => r.text,
            Err(e) => { eprintln!("[review] Parakeet ASR failed: {e}"); String::new() }
        }
    };

    // Run forced alignment on ASR outputs too (for time-based grouping)
    let align_to_json = |text: &str| -> Vec<serde_json::Value> {
        if text.is_empty() { return vec![]; }
        match state.aligner.align(&samples_16k, text) {
            Ok(items) => items.iter().map(|item| serde_json::json!({
                "word": item.word, "start": item.start_time, "end": item.end_time,
            })).collect(),
            Err(e) => { eprintln!("[review] Aligner failed on ASR text: {e}"); vec![] }
        }
    };
    let qwen_alignment = align_to_json(&qwen_asr);
    let parakeet_alignment = align_to_json(&parakeet_asr);

    // Encode audio as base64
    use base64::Engine;
    let audio_b64 = base64::engine::general_purpose::STANDARD.encode(&wav_bytes);

    // Store in DB
    let alignment_json = serde_json::to_string(&alignment)?;
    {
        let db = state.db.lock().unwrap();
        db.update_sentence_precomputed(
            sentence.id, &wav_path, &alignment_json, backend, &sentence.spoken,
        )?;
    }

    Ok(PrecomputedData { audio_b64, alignment, written_alignment, qwen_alignment, parakeet_alignment, wav_path, qwen_asr, parakeet_asr })
}

/// Build the full JSON response for a review screen.
fn build_review_response(
    state: &Arc<AppState>,
    sentence: &SentenceRow,
    precomputed: &PrecomputedData,
    backend: &str,
) -> serde_json::Value {
    let backends = state.tts.available_backends();
    let unknown_words: Vec<String> = serde_json::from_str(&sentence.unknown_words).unwrap_or_default();

    let (approved, rejected, total) = {
        let db = state.db.lock().unwrap();
        db.sentence_count_by_status().unwrap_or((0, 0, 0))
    };
    let reviewed = approved + rejected;
    let remaining = total - reviewed;

    serde_json::json!({
        "sentence": {
            "id": sentence.id,
            "text": sentence.text,
            "spoken": sentence.spoken,
            "unknown_words": unknown_words,
            "status": sentence.status,
        },
        "audio_b64": precomputed.audio_b64,
        "alignment": precomputed.alignment,
        "written_alignment": precomputed.written_alignment,
        "asr": {
            "qwen": precomputed.qwen_asr,
            "parakeet": precomputed.parakeet_asr,
            "qwen_alignment": precomputed.qwen_alignment,
            "parakeet_alignment": precomputed.parakeet_alignment,
        },
        "backend": backend,
        "backends": backends,
        "progress": {
            "reviewed": reviewed,
            "total": total,
            "remaining": remaining,
        },
        "ready": true,
    })
}

/// Ensure the review session has a current sentence and queue is populated.
/// Returns the current sentence ID or None if nothing left.
fn ensure_current(state: &Arc<AppState>) -> Option<i64> {
    let mut review = state.review.lock().unwrap();

    // If current sentence is gone or already reviewed, clear it
    if let Some(id) = review.current_id {
        let db = state.db.lock().unwrap();
        match db.get_sentence(id) {
            Ok(Some(s)) if s.status == "pending" => return Some(id),
            _ => { review.current_id = None; }
        }
    }

    // Pop from queue until we find a valid pending sentence
    while let Some(id) = review.queue.pop_front() {
        let db = state.db.lock().unwrap();
        if let Ok(Some(s)) = db.get_sentence(id) {
            if s.status == "pending" {
                review.current_id = Some(id);
                return Some(id);
            }
        }
    }

    // Queue empty — refill from DB
    {
        let db = state.db.lock().unwrap();
        // Auto-promote candidates if needed
        let pending_count = db.pending_sentence_ids(1).map(|v| v.len()).unwrap_or(0);
        if pending_count == 0 {
            let candidates = db.pick_candidates(50, true).unwrap_or_default();
            for (text, spoken, vocab_terms, unknown_words) in &candidates {
                let _ = db.insert_sentence_from_candidate(text, spoken, vocab_terms, unknown_words);
            }
        }

        if let Ok(ids) = db.pending_sentence_ids(50) {
            for id in ids {
                review.queue.push_back(id);
            }
        }
    }

    // Try again
    if let Some(id) = review.queue.pop_front() {
        review.current_id = Some(id);
        Some(id)
    } else {
        None
    }
}

// ==================== Background Pre-computation ====================

pub fn spawn_precompute_loop(state: Arc<AppState>, notify: Arc<Notify>, audio_dir: String) {
    tokio::spawn(async move {
        loop {
            notify.notified().await;

            // Grab the next few IDs that need precomputation
            let (ids_to_compute, backend) = {
                let review = state.review.lock().unwrap();
                let backend = review.backend.clone();
                let mut ids = Vec::new();
                for id in &review.queue {
                    if !review.precomputed.contains_key(id) && ids.len() < 3 {
                        ids.push(*id);
                    }
                }
                (ids, backend)
            };

            for id in ids_to_compute {
                let sentence = {
                    let db = state.db.lock().unwrap();
                    db.get_sentence(id).ok().flatten()
                };
                let Some(sentence) = sentence else { continue };
                if sentence.status != "pending" { continue; }

                let state2 = state.clone();
                let backend2 = backend.clone();
                let audio_dir2 = audio_dir.clone();

                // Run TTS + alignment on blocking thread
                let result = tokio::task::spawn_blocking(move || {
                    compute_for_sentence(&state2, &sentence, &backend2, &audio_dir2)
                }).await;

                match result {
                    Ok(Ok(data)) => {
                        eprintln!("[precompute] sentence {} ready", id);
                        let mut review = state.review.lock().unwrap();
                        review.precomputed.insert(id, data);
                    }
                    Ok(Err(e)) => {
                        eprintln!("[precompute] sentence {} failed: {e}", id);
                    }
                    Err(e) => {
                        eprintln!("[precompute] sentence {} task failed: {e}", id);
                    }
                }
            }
        }
    });
}

// ==================== API Endpoints ====================

pub async fn api_review_current(
    State(state): State<Arc<AppState>>,
) -> Result<Response, AppError> {
    let Some(id) = ensure_current(&state) else {
        return Ok(Json(serde_json::json!({
            "sentence": null,
            "ready": true,
        })).into_response());
    };

    // Check if we have precomputed data
    let precomputed = {
        let mut review = state.review.lock().unwrap();
        review.precomputed.remove(&id)
    };

    if let Some(data) = precomputed {
        let sentence = {
            let db = state.db.lock().unwrap();
            db.get_sentence(id).map_err(err)?.ok_or_else(|| err(anyhow::anyhow!("sentence gone")))?
        };
        let backend = state.review.lock().unwrap().backend.clone();
        let response = build_review_response(&state, &sentence, &data, &backend);

        // Trigger precomputation of next sentences
        state.precompute_notify.notify_one();

        return Ok(Json(response).into_response());
    }

    // Not precomputed — compute synchronously (cold start)
    let sentence = {
        let db = state.db.lock().unwrap();
        db.get_sentence(id).map_err(err)?.ok_or_else(|| err(anyhow::anyhow!("sentence gone")))?
    };
    let backend = state.review.lock().unwrap().backend.clone();
    let audio_dir = state.audio_dir.clone();

    let state2 = state.clone();
    let backend2 = backend.clone();
    let data = tokio::task::spawn_blocking(move || {
        compute_for_sentence(&state2, &sentence, &backend2, &audio_dir)
    })
    .await
    .map_err(|e| err(e))?
    .map_err(err)?;

    // Re-read sentence (may have been updated by compute)
    let sentence = {
        let db = state.db.lock().unwrap();
        db.get_sentence(id).map_err(err)?.ok_or_else(|| err(anyhow::anyhow!("sentence gone")))?
    };

    let response = build_review_response(&state, &sentence, &data, &backend);

    // Trigger precomputation of next sentences
    state.precompute_notify.notify_one();

    Ok(Json(response).into_response())
}

pub async fn api_review_approve(
    State(state): State<Arc<AppState>>,
) -> Result<Response, AppError> {
    let id = {
        let review = state.review.lock().unwrap();
        review.current_id
    };
    let Some(id) = id else {
        return Ok(Json(serde_json::json!({"error": "no current sentence"})).into_response());
    };

    {
        let db = state.db.lock().unwrap();
        db.update_sentence_status(id, "approved").map_err(err)?;
    }

    // Advance to next
    {
        let mut review = state.review.lock().unwrap();
        review.current_id = None;
        review.precomputed.remove(&id);
    }

    // Return the next sentence
    api_review_current(State(state)).await
}

pub async fn api_review_reject(
    State(state): State<Arc<AppState>>,
) -> Result<Response, AppError> {
    let id = {
        let review = state.review.lock().unwrap();
        review.current_id
    };
    let Some(id) = id else {
        return Ok(Json(serde_json::json!({"error": "no current sentence"})).into_response());
    };

    {
        let db = state.db.lock().unwrap();
        db.update_sentence_status(id, "rejected").map_err(err)?;
    }

    // Advance to next
    {
        let mut review = state.review.lock().unwrap();
        review.current_id = None;
        review.precomputed.remove(&id);
    }

    api_review_current(State(state)).await
}

#[derive(Deserialize)]
pub struct PronunciationBody {
    word: String,
    spoken: String,
}

pub async fn api_review_pronunciation(
    State(state): State<Arc<AppState>>,
    Json(body): Json<PronunciationBody>,
) -> Result<Response, AppError> {
    let id = {
        let review = state.review.lock().unwrap();
        review.current_id
    };
    let Some(id) = id else {
        return Ok(Json(serde_json::json!({"error": "no current sentence"})).into_response());
    };

    // Update vocab override
    {
        let db = state.db.lock().unwrap();
        match db.find_vocab_by_term(&body.word) {
            Ok(Some(vocab)) => {
                eprintln!("[pronunciation] updating vocab '{}' (id={}) → '{}'", vocab.term, vocab.id, body.spoken);
                db.update_vocab_override(vocab.id, Some(&body.spoken)).map_err(err)?;
            }
            Ok(None) => {
                eprintln!("[pronunciation] vocab entry '{}' not found, inserting", body.word);
                let _ = db.insert_candidate_vocab(&body.word, &body.spoken);
                // Now update the override
                if let Ok(Some(vocab)) = db.find_vocab_by_term(&body.word) {
                    db.update_vocab_override(vocab.id, Some(&body.spoken)).map_err(err)?;
                }
            }
            Err(e) => eprintln!("[pronunciation] error finding vocab: {e}"),
        }
    }

    // Rebuild spoken form for current sentence and all queued sentences containing this word
    let ids_to_update = {
        let review = state.review.lock().unwrap();
        let mut ids = vec![id];
        ids.extend(review.queue.iter());
        ids
    };
    {
        let db = state.db.lock().unwrap();
        for sid in ids_to_update {
            if let Ok(Some(s)) = db.get_sentence(sid) {
                let new_spoken = tts::replace_word_in_spoken(&s.spoken, &body.word, &body.spoken);
                eprintln!("[pronunciation] sentence {sid}: '{}' → '{}'", s.spoken, new_spoken);
                if new_spoken != s.spoken {
                    let _ = db.update_sentence_spoken(sid, &new_spoken);
                    let _ = db.update_sentence_status(sid, "pending");
                }
            }
        }
    }

    // Invalidate all precomputed data (spoken forms changed)
    {
        let mut review = state.review.lock().unwrap();
        review.precomputed.clear();
    }

    // Re-read updated sentence, re-compute TTS + alignment
    let sentence = {
        let db = state.db.lock().unwrap();
        db.get_sentence(id).map_err(err)?.ok_or_else(|| err(anyhow::anyhow!("sentence gone")))?
    };
    let backend = state.review.lock().unwrap().backend.clone();
    let audio_dir = state.audio_dir.clone();
    let state2 = state.clone();
    let backend2 = backend.clone();

    let data = tokio::task::spawn_blocking(move || {
        compute_for_sentence(&state2, &sentence, &backend2, &audio_dir)
    })
    .await
    .map_err(|e| err(e))?
    .map_err(err)?;

    let sentence = {
        let db = state.db.lock().unwrap();
        db.get_sentence(id).map_err(err)?.ok_or_else(|| err(anyhow::anyhow!("sentence gone")))?
    };

    let response = build_review_response(&state, &sentence, &data, &backend);

    // Trigger precomputation for queued sentences (with updated spoken forms)
    state.precompute_notify.notify_one();

    Ok(Json(response).into_response())
}

#[derive(Deserialize)]
pub struct EditTextBody {
    text: String,
}

/// Edit the sentence text (fix transcription errors). Rebuilds spoken form using vocab overrides, re-synths.
pub async fn api_review_edit_text(
    State(state): State<Arc<AppState>>,
    Json(body): Json<EditTextBody>,
) -> Result<Response, AppError> {
    let id = {
        let review = state.review.lock().unwrap();
        review.current_id
    };
    let Some(id) = id else {
        return Ok(Json(serde_json::json!({"error": "no current sentence"})).into_response());
    };

    // Update the text and rebuild spoken form using existing vocab overrides
    let new_text = body.text.trim().to_string();
    {
        let db = state.db.lock().unwrap();
        let overrides = db.get_spoken_overrides().map_err(err)?;

        // Start with the new text as spoken, then apply overrides
        let mut spoken = new_text.clone();
        for (term, spoken_form) in &overrides {
            spoken = tts::replace_word_in_spoken(&spoken, term, spoken_form);
        }

        // Update text, spoken, and unknown words
        let unknown = crate::tts::detect_unknown_words(&new_text);
        let unknown_json = serde_json::to_string(&unknown).unwrap_or_default();
        db.update_sentence_text(id, &new_text, &spoken, &unknown_json).map_err(err)?;
    }

    // Invalidate precomputed data
    {
        let mut review = state.review.lock().unwrap();
        review.precomputed.clear();
    }

    // Re-compute TTS + alignment
    let sentence = {
        let db = state.db.lock().unwrap();
        db.get_sentence(id).map_err(err)?.ok_or_else(|| err(anyhow::anyhow!("sentence gone")))?
    };
    let backend = state.review.lock().unwrap().backend.clone();
    let audio_dir = state.audio_dir.clone();
    let state2 = state.clone();
    let backend2 = backend.clone();

    let data = tokio::task::spawn_blocking(move || {
        compute_for_sentence(&state2, &sentence, &backend2, &audio_dir)
    })
    .await
    .map_err(|e| err(e))?
    .map_err(err)?;

    let sentence = {
        let db = state.db.lock().unwrap();
        db.get_sentence(id).map_err(err)?.ok_or_else(|| err(anyhow::anyhow!("sentence gone")))?
    };

    let response = build_review_response(&state, &sentence, &data, &backend);
    state.precompute_notify.notify_one();
    Ok(Json(response).into_response())
}

#[derive(Deserialize)]
pub struct BackendBody {
    backend: String,
}

pub async fn api_review_backend(
    State(state): State<Arc<AppState>>,
    Json(body): Json<BackendBody>,
) -> Result<Response, AppError> {
    // Update backend and invalidate precomputed cache
    {
        let mut review = state.review.lock().unwrap();
        review.backend = body.backend.clone();
        review.precomputed.clear();
    }

    let id = {
        let review = state.review.lock().unwrap();
        review.current_id
    };
    let Some(id) = id else {
        return Ok(Json(serde_json::json!({"error": "no current sentence"})).into_response());
    };

    // Re-compute current sentence with new backend
    let sentence = {
        let db = state.db.lock().unwrap();
        db.get_sentence(id).map_err(err)?.ok_or_else(|| err(anyhow::anyhow!("sentence gone")))?
    };
    let backend = body.backend.clone();
    let audio_dir = state.audio_dir.clone();
    let state2 = state.clone();
    let backend2 = backend.clone();

    let data = tokio::task::spawn_blocking(move || {
        compute_for_sentence(&state2, &sentence, &backend2, &audio_dir)
    })
    .await
    .map_err(|e| err(e))?
    .map_err(err)?;

    let sentence = {
        let db = state.db.lock().unwrap();
        db.get_sentence(id).map_err(err)?.ok_or_else(|| err(anyhow::anyhow!("sentence gone")))?
    };

    let response = build_review_response(&state, &sentence, &data, &backend);

    // Trigger precomputation with new backend
    state.precompute_notify.notify_one();

    Ok(Json(response).into_response())
}

/// Run both ASR models on uploaded audio, then align the ASR text against the
/// TTS waveform so the transcript grid can display time-aligned human ASR results.
pub async fn api_review_asr(
    State(state): State<Arc<AppState>>,
    body: axum::body::Bytes,
) -> Result<Response, AppError> {
    let current_id = state.review.lock().unwrap().current_id;
    let audio_dir = state.audio_dir.clone();
    let state2 = state.clone();

    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<serde_json::Value> {
        let wav_bytes = body.to_vec();

        // Save human recording to disk for eval
        if let Some(id) = current_id {
            let human_wav_path = format!("{}/human_{}.wav", audio_dir, id);
            if let Err(e) = std::fs::write(&human_wav_path, &wav_bytes) {
                eprintln!("[human-asr] Failed to save recording: {e}");
            } else {
                let db = state2.db.lock().unwrap();
                let _ = db.update_sentence_human_wav(id, &human_wav_path);
                eprintln!("[human-asr] Saved recording to {human_wav_path}");
            }
        }

        // Decode human recording WAV
        let cursor = std::io::Cursor::new(wav_bytes);
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

        let samples_16k = crate::tts::resample_to_16k(&mono, spec.sample_rate)?;

        // Run both ASR models on human recording
        let qwen = match state2.asr.transcribe_samples(&samples_16k, qwen3_asr::TranscribeOptions::default()) {
            Ok(r) => r.text,
            Err(e) => format!("(error: {e})"),
        };

        let parakeet = {
            let mut p = state2.parakeet.lock().unwrap();
            match p.transcribe_samples(samples_16k.to_vec(), 16000, 1, None) {
                Ok(r) => r.text,
                Err(e) => format!("(error: {e})"),
            }
        };

        // Run forced aligner on TTS audio with the human ASR text, so the
        // transcript grid can show time-aligned human ASR words against the TTS waveform
        let (qwen_alignment, parakeet_alignment) = if let Some(id) = current_id {
            let wav_path = format!("{}/{}.wav", audio_dir, id);
            let tts_16k = load_wav_16k(&wav_path);
            match tts_16k {
                Ok(samples) => {
                    let align = |text: &str| -> Vec<serde_json::Value> {
                        if text.is_empty() { return vec![]; }
                        match state2.aligner.align(&samples, text) {
                            Ok(items) => items.iter().map(|item| serde_json::json!({
                                "word": item.word, "start": item.start_time, "end": item.end_time,
                            })).collect(),
                            Err(e) => { eprintln!("[human-asr] Aligner failed: {e}"); vec![] }
                        }
                    };
                    (align(&qwen), align(&parakeet))
                }
                Err(e) => { eprintln!("[human-asr] Failed to load TTS wav: {e}"); (vec![], vec![]) }
            }
        } else {
            (vec![], vec![])
        };

        Ok(serde_json::json!({
            "qwen": qwen,
            "parakeet": parakeet,
            "qwen_alignment": qwen_alignment,
            "parakeet_alignment": parakeet_alignment,
        }))
    })
    .await
    .map_err(|e| err(e))?
    .map_err(err)?;

    Ok(Json(result).into_response())
}

/// Load a WAV file from disk and resample to 16kHz mono.
fn load_wav_16k(wav_path: &str) -> anyhow::Result<Vec<f32>> {
    let mut reader = hound::WavReader::open(wav_path)
        .map_err(|e| anyhow::anyhow!("WAV open {wav_path}: {e}"))?;
    let spec = reader.spec();
    let samples_f32: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().filter_map(|s| s.ok()).collect(),
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
    tts::resample_to_16k(&mono, spec.sample_rate)
}
