use anyhow::Result;

/// Raw audio output from a TTS backend
pub struct TtsAudio {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
}

impl TtsAudio {
    /// Peak-normalize to -1 dB
    pub fn normalize(&mut self) {
        let peak = self.samples.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        if peak > 0.0 {
            let target = 10.0f32.powf(-1.0 / 20.0);
            let gain = target / peak;
            for s in &mut self.samples {
                *s *= gain;
            }
        }
    }

    /// Encode as WAV bytes
    pub fn to_wav(&self) -> Result<Vec<u8>> {
        let mut cursor = std::io::Cursor::new(Vec::new());
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: self.sample_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::new(&mut cursor, spec)?;
        for &s in &self.samples {
            let clamped = (s * 32767.0f32).clamp(-32768.0, 32767.0);
            writer.write_sample(clamped as i16)?;
        }
        writer.finalize()?;
        Ok(cursor.into_inner())
    }
}

// ==================== Local (sync) backends ====================

/// Sync TTS backend — runs on a blocking thread
trait LocalTtsBackend: Send + 'static {
    fn name(&self) -> &'static str;
    fn generate(&mut self, text: &str) -> Result<TtsAudio>;
}

struct PocketTtsBackend {
    model: pocket_tts::TTSModel,
    voice_state: pocket_tts::ModelState,
    sample_rate: u32,
}

impl LocalTtsBackend for PocketTtsBackend {
    fn name(&self) -> &'static str { "pocket" }

    fn generate(&mut self, text: &str) -> Result<TtsAudio> {
        let audio = self.model.generate(text, &self.voice_state)
            .map_err(|e| anyhow::anyhow!("pocket-tts: {e}"))?;
        let samples: Vec<f32> = audio.flatten_all()?.to_vec1()?;
        Ok(TtsAudio { samples, sample_rate: self.sample_rate })
    }
}

struct KokoroBackend {
    model: voice_tts::KokoroModel,
    voice: mlx_rs::Array,
}

impl LocalTtsBackend for KokoroBackend {
    fn name(&self) -> &'static str { "kokoro" }

    fn generate(&mut self, text: &str) -> Result<TtsAudio> {
        let phonemes = voice_g2p::english_to_phonemes(text)
            .map_err(|e| anyhow::anyhow!("g2p: {e}"))?;
        let audio = voice_tts::generate(&mut self.model, &phonemes, &self.voice, 1.0)
            .map_err(|e| anyhow::anyhow!("kokoro: {e}"))?;
        audio.eval().map_err(|e| anyhow::anyhow!("mlx eval: {e}"))?;
        let samples: Vec<f32> = audio.as_slice().to_vec();
        Ok(TtsAudio { samples, sample_rate: 24000 })
    }
}

// ==================== Remote (async) backends ====================

/// Async TTS backend — makes HTTP calls, no mutable state needed
trait RemoteTtsBackend: Send + Sync + 'static {
    fn name(&self) -> &'static str;
    fn generate(&self, text: &str) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<TtsAudio>> + Send + '_>>;
}

struct OpenAiTtsBackend {
    api_key: String,
    client: reqwest::Client,
}

impl RemoteTtsBackend for OpenAiTtsBackend {
    fn name(&self) -> &'static str { "openai" }

    fn generate(&self, text: &str) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<TtsAudio>> + Send + '_>> {
        let text = text.to_string();
        Box::pin(async move {
            let resp = self.client
                .post("https://api.openai.com/v1/audio/speech")
                .bearer_auth(&self.api_key)
                .json(&serde_json::json!({
                    "model": "tts-1-hd",
                    "input": text,
                    "voice": "onyx",
                    "response_format": "wav",
                }))
                .send()
                .await?;

            if !resp.status().is_success() {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                anyhow::bail!("OpenAI TTS {status}: {body}");
            }

            let wav_bytes = resp.bytes().await?;
            decode_wav(&wav_bytes)
        })
    }
}

struct ElevenLabsTtsBackend {
    api_key: String,
    voice_id: String,
    client: reqwest::Client,
}

impl RemoteTtsBackend for ElevenLabsTtsBackend {
    fn name(&self) -> &'static str { "elevenlabs" }

    fn generate(&self, text: &str) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<TtsAudio>> + Send + '_>> {
        let text = text.to_string();
        Box::pin(async move {
            let url = format!("https://api.elevenlabs.io/v1/text-to-speech/{}", self.voice_id);
            let resp = self.client
                .post(&url)
                .header("xi-api-key", &self.api_key)
                .json(&serde_json::json!({
                    "text": text,
                    "model_id": "eleven_turbo_v2_5",
                    "output_format": "pcm_24000",
                }))
                .send()
                .await?;

            if !resp.status().is_success() {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                anyhow::bail!("ElevenLabs TTS {status}: {body}");
            }

            let pcm_bytes = resp.bytes().await?;
            let samples: Vec<f32> = pcm_bytes
                .chunks_exact(2)
                .map(|chunk| {
                    let sample = i16::from_le_bytes([chunk[0], chunk[1]]);
                    sample as f32 / 32768.0
                })
                .collect();

            Ok(TtsAudio { samples, sample_rate: 24000 })
        })
    }
}

// ==================== Helpers ====================

fn decode_wav(wav_bytes: &[u8]) -> Result<TtsAudio> {
    let cursor = std::io::Cursor::new(wav_bytes);
    let mut reader = hound::WavReader::new(cursor)?;
    let spec = reader.spec();
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => {
            reader.samples::<f32>().filter_map(|s| s.ok()).collect()
        }
        hound::SampleFormat::Int => {
            let max = (1i64 << (spec.bits_per_sample - 1)) as f32;
            reader.samples::<i32>().filter_map(|s| s.ok()).map(|s| s as f32 / max).collect()
        }
    };
    Ok(TtsAudio { samples, sample_rate: spec.sample_rate })
}

// ==================== Manager ====================

use std::sync::Mutex;

/// Holds all TTS backends. Local backends are behind Mutex (sync, need &mut).
/// Remote backends are shared (async, &self only).
pub struct TtsManager {
    local: Vec<Mutex<Box<dyn LocalTtsBackend>>>,
    remote: Vec<Box<dyn RemoteTtsBackend>>,
}

impl TtsManager {
    pub fn available_backends(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.local.iter()
            .map(|b| b.lock().unwrap().name())
            .collect();
        names.extend(self.remote.iter().map(|b| b.name()));
        names
    }

    /// Generate audio. Local backends run on spawn_blocking, remote backends run async.
    pub async fn generate(&self, backend_name: &str, text: &str) -> Result<TtsAudio> {
        // Check local backends
        for local in &self.local {
            let name = local.lock().unwrap().name();
            if name == backend_name {
                let text = text.to_string();
                // Move the mutex ref into spawn_blocking via a pointer trick —
                // actually, we can't move &Mutex into spawn_blocking.
                // Instead, lock, generate, unlock — all sync, no await.
                let mut backend = local.lock().unwrap();
                return backend.generate(&text);
            }
        }

        // Check remote backends
        for remote in &self.remote {
            if remote.name() == backend_name {
                return remote.generate(text).await;
            }
        }

        anyhow::bail!("TTS backend '{backend_name}' not available")
    }
}

/// Build a TtsManager with all available backends
pub fn init(voice_path: &str, kokoro_voice: &str) -> TtsManager {
    let mut local: Vec<Mutex<Box<dyn LocalTtsBackend>>> = Vec::new();
    let mut remote: Vec<Box<dyn RemoteTtsBackend>> = Vec::new();

    // Pocket-tts
    match PocketTtsBackend::load(voice_path) {
        Ok(b) => local.push(Mutex::new(Box::new(b))),
        Err(e) => eprintln!("pocket-tts not available: {e}"),
    }

    // Kokoro
    match KokoroBackend::load(kokoro_voice) {
        Ok(b) => local.push(Mutex::new(Box::new(b))),
        Err(e) => eprintln!("Kokoro not available: {e}"),
    }

    // OpenAI
    if std::env::var("OPENAI_API_KEY").is_ok() {
        let api_key = std::env::var("OPENAI_API_KEY").unwrap();
        remote.push(Box::new(OpenAiTtsBackend {
            api_key,
            client: reqwest::Client::new(),
        }));
        eprintln!("OpenAI TTS ready");
    }

    // ElevenLabs
    if std::env::var("ELEVENLABS_API_KEY").is_ok() {
        let api_key = std::env::var("ELEVENLABS_API_KEY").unwrap();
        let voice_id = std::env::var("ELEVENLABS_VOICE_ID")
            .unwrap_or_else(|_| "21m00Tcm4TlvDq8ikWAM".to_string());
        remote.push(Box::new(ElevenLabsTtsBackend {
            api_key,
            voice_id,
            client: reqwest::Client::new(),
        }));
        eprintln!("ElevenLabs TTS ready");
    }

    TtsManager { local, remote }
}

// Keep PocketTtsBackend::load and KokoroBackend::load as private helpers
impl PocketTtsBackend {
    fn load(voice_path: &str) -> Result<Self> {
        eprintln!("Loading pocket-tts (quantized)...");
        let model = pocket_tts::TTSModel::load_quantized("b6369a24")?;
        let voice_state = model
            .get_voice_state(voice_path)
            .map_err(|e| anyhow::anyhow!("loading voice '{voice_path}': {e}"))?;
        let sample_rate = model.sample_rate as u32;
        eprintln!("pocket-tts ready ({sample_rate} Hz)");
        Ok(Self { model, voice_state, sample_rate })
    }
}

impl KokoroBackend {
    fn load(voice_name: &str) -> Result<Self> {
        eprintln!("Loading Kokoro TTS...");
        let model = voice_tts::load_model("prince-canuma/Kokoro-82M")
            .map_err(|e| anyhow::anyhow!("kokoro model: {e}"))?;
        let voice = voice_tts::load_voice(voice_name, None)
            .map_err(|e| anyhow::anyhow!("kokoro voice '{voice_name}': {e}"))?;
        eprintln!("Kokoro ready (24000 Hz, voice: {voice_name})");
        Ok(Self { model, voice })
    }
}
