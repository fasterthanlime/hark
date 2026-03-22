import Foundation

/// Available Qwen3 ASR model variants (used via Rust qwen3-asr inference).
struct STTModelDefinition: Identifiable, Hashable {
    let id: String
    let displayName: String
    let repoID: String

    static let allModels: [STTModelDefinition] = [
        STTModelDefinition(
            id: "qwen3-0.6b",
            displayName: "Qwen3 ASR 0.6B",
            repoID: "Qwen/Qwen3-ASR-0.6B"
        ),
        STTModelDefinition(
            id: "qwen3-1.7b",
            displayName: "Qwen3 ASR 1.7B",
            repoID: "Qwen/Qwen3-ASR-1.7B"
        ),
    ]

    static let `default` = allModels[0]

    /// Cache directory for model files.
    static var cacheDirectory: String {
        let caches = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask).first!
        return caches.appendingPathComponent("qwen3-asr").path
    }
}
