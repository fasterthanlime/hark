#[vox::service]
pub trait BeeMl {
    async fn transcribe_wav(&self, wav_bytes: Vec<u8>) -> Result<String, String>;
}
