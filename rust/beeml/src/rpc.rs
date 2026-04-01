use facet::Facet;
use vox::{Rx, Tx};

use bee_transcribe::{AlignedWord, Update};

#[derive(Clone, Debug, Facet)]
pub struct TranscribeWavResult {
    pub transcript: String,
    pub words: Vec<AlignedWord>,
}

#[vox::service]
pub trait BeeMl {
    async fn transcribe_wav(&self, wav_bytes: Vec<u8>) -> Result<TranscribeWavResult, String>;

    /// Stream audio chunks (16kHz mono f32) and receive incremental transcription updates.
    async fn stream_transcribe(
        &self,
        audio_in: Rx<Vec<f32>>,
        updates_out: Tx<Update>,
    ) -> Result<(), String>;
}
