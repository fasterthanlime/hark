import Foundation
import os

private let logger = Logger(subsystem: "fasterthanlime.bee", category: "Session")

/// A Session is a self-contained unit of work for a single dictation attempt.
/// It owns three layers — Capture, ASR, and IME — each with its own state.
/// Multiple sessions can coexist (e.g., the previous one finalizing while a
/// new one is streaming).
actor Session {
    let id: UUID
    let targetBundleID: String?
    let createdAt: Date

    private(set) var capture: CaptureState = .buffering
    private(set) var asr: ASRState = .idle
    private(set) var ime: IMEState = .inactive

    private let audioEngine: AudioEngine
    private let transcriptionService: TranscriptionService
    private let inputClient: BeeInputClient

    private var processedNativeCount: Int = 0
    private var asrSession: StreamingSession?
    private var streamingTask: Task<Void, Never>?
    private var finalText: String = ""
    private var partialTranscript: String = ""

    private var onStreamingUpdate: (@Sendable (String) -> Void)?
    private var onComplete: (@Sendable (SessionResult) -> Void)?

    /// Diagnostics — populated during the session lifecycle
    private(set) var diag = SessionDiagnostics()

    func setOnComplete(_ handler: @Sendable @escaping (SessionResult) -> Void) {
        onComplete = handler
    }

    func setOnStreamingUpdate(_ handler: @Sendable @escaping (String) -> Void) {
        onStreamingUpdate = handler
    }

    init(
        audioEngine: AudioEngine,
        transcriptionService: TranscriptionService,
        inputClient: BeeInputClient,
        targetBundleID: String?
    ) {
        self.id = UUID()
        self.createdAt = Date()
        self.audioEngine = audioEngine
        self.transcriptionService = transcriptionService
        self.inputClient = inputClient
        self.targetBundleID = targetBundleID
    }

    // MARK: - Starting

    func start(language: String?) async {
        logger.info("[\(self.id)] Session starting")

        // Warm up engine if cold
        if !audioEngine.isWarm {
            do {
                try audioEngine.warmUp()
            } catch {
                logger.error("[\(self.id)] Failed to warm up audio engine: \(error)")
                onComplete?(.aborted(id: id))
                return
            }
        }

        // Start capturing audio (copies pre-buffer)
        capture = .buffering
        diag.startedAt = Date()
        diag.nativeRate = audioEngine.nativeSampleRate
        audioEngine.startCapture(for: self.id)

        // Activate IME (TIS calls need main thread)
        await MainActor.run { inputClient.activate() }
        ime = .active

        // Create ASR session
        asrSession = transcriptionService.createSession(language: language)
        if asrSession != nil {
            asr = .streaming
        } else {
            logger.error("[\(self.id)] Failed to create ASR session")
        }

        // Start the streaming loop
        streamingTask = Task { [weak self] in
            await self?.streamingLoop()
        }

        logger.info("[\(self.id)] Session started, streaming")
    }

    // MARK: - Ending

    /// Immediate teardown. No finalization, no history, no trace.
    func abort() async {
        guard !isTerminal else { return }
        logger.info("[\(self.id)] Aborting")

        streamingTask?.cancel()
        audioEngine.cancelCapture(for: self.id)
        capture = .discarded
        asrSession = nil
        asr = .done

        // Deactivate IME — session was never visible
        await MainActor.run { inputClient.deactivate() }
        ime = .tornDown

        onComplete?(.aborted(id: id))
    }

    /// Finalize in background, create history entry, but don't insert text.
    func cancel() async {
        guard !isTerminal else { return }
        logger.info("[\(self.id)] Cancelling")

        streamingTask?.cancel()
        diag.endedAt = Date()
        diag.ending = "cancel"

        capture = .draining
        let samples = audioEngine.stopCapture(for: self.id)
        capture = .delivered
        diag.totalNativeSamples = samples.count

        // Clear marked text and deactivate IME
        inputClient.clearMarkedText()
        await MainActor.run { inputClient.deactivate() }
        ime = .cleared

        Task.detached { [self] in
            await self.finalize(samples: samples, insert: false, submit: false)
        }
    }

    /// Finalize and insert text. If submit, simulate Return after insertion.
    func commit(submit: Bool) async {
        guard !isTerminal else { return }
        logger.info("[\(self.id)] Committing (submit=\(submit))")

        streamingTask?.cancel()
        diag.endedAt = Date()
        diag.ending = submit ? "commit+submit" : "commit"

        capture = .draining
        let samples = audioEngine.stopCapture(for: self.id)
        capture = .delivered
        diag.totalNativeSamples = samples.count

        Task.detached { [self] in
            await self.finalize(samples: samples, insert: true, submit: submit)
        }
    }

    // MARK: - Internal

    private var isTerminal: Bool {
        switch capture {
        case .discarded: return true
        default: break
        }
        switch (asr, ime) {
        case (.done, .committed): return true
        case (.done, .cleared): return true
        case (.done, .tornDown): return true
        default: return false
        }
    }

    private func streamingLoop() async {
        guard let session = asrSession else { return }
        let nativeRate = audioEngine.nativeSampleRate
        // Min chunk in native samples (~50ms)
        let minNativeChunk = Int(nativeRate * 0.05)

        while !Task.isCancelled && capture == .buffering {
            let allNative = audioEngine.peekCapture(for: self.id)
            let newNativeCount = allNative.count

            guard newNativeCount > processedNativeCount + minNativeChunk else {
                try? await Task.sleep(for: .milliseconds(30))
                continue
            }

            // Slice new native-rate samples, resample to 16kHz, feed to ASR
            let nativeChunk = Array(allNative[processedNativeCount...])
            processedNativeCount = newNativeCount
            let resampled = AudioEngine.resample(nativeChunk, from: nativeRate)

            diag.streamingFeeds += 1
            diag.streamedNativeSamples = processedNativeCount
            diag.streamedResampledSamples += resampled.count

            if let update = transcriptionService.feed(session: session, samples: resampled) {
                partialTranscript = update.text
                inputClient.setMarkedText(update.text)
                onStreamingUpdate?(update.text)
            }
        }
    }

    private func finalize(samples: [Float], insert: Bool, submit: Bool) async {
        asr = .finalizing

        guard let session = asrSession else {
            asr = .done
            finalText = partialTranscript
            await completeSession(insert: insert, submit: submit)
            return
        }

        // Feed remaining unprocessed native samples, resampled to 16kHz
        let nativeRate = audioEngine.nativeSampleRate
        let remainingNative = samples.count > processedNativeCount
            ? Array(samples[processedNativeCount...])
            : []

        diag.remainingNativeSamples = remainingNative.count
        diag.finalizeStartedAt = Date()

        if !remainingNative.isEmpty {
            var resampled = AudioEngine.resample(remainingNative, from: nativeRate)
            diag.remainingResampledSamples = resampled.count

            // Add silence padding for trailing speech (100ms at 16kHz)
            let padSamples = Int(AudioEngine.targetSampleRate * 0.1)
            resampled.append(contentsOf: repeatElement(Float(0), count: padSamples))

            if let update = transcriptionService.feedFinalizing(session: session, samples: resampled) {
                partialTranscript = update.text
            }
        }

        // Run final inference
        if let text = transcriptionService.finish(session: session) {
            let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
            if !trimmed.isEmpty {
                finalText = trimmed
            } else {
                finalText = partialTranscript
            }
        } else {
            finalText = partialTranscript
        }

        asr = .done
        asrSession = nil

        diag.finalizeEndedAt = Date()
        diag.finalText = finalText

        // Save full audio as WAV for debugging
        let allResampled = AudioEngine.resample(samples, from: nativeRate)
        let debugDir = FileManager.default.temporaryDirectory.appendingPathComponent("bee-debug")
        try? FileManager.default.createDirectory(at: debugDir, withIntermediateDirectories: true)
        let wavURL = debugDir.appendingPathComponent("\(id.uuidString.prefix(8)).wav")
        try? WavWriter.write(samples: allResampled, to: wavURL)
        diag.audioWavPath = wavURL.path
        logger.info("[\(self.id)] Saved audio to \(wavURL.path)")

        logger.info("[\(self.id)] Finalized: \"\(self.finalText.prefix(80))\"")
        await completeSession(insert: insert, submit: submit)
    }

    private func completeSession(insert: Bool, submit: Bool) async {
        if insert && !finalText.isEmpty {
            // Commit text via IME
            inputClient.commitText(finalText)
            try? await Task.sleep(for: .milliseconds(50))
            await MainActor.run { inputClient.deactivate() }
            ime = .committed

            if submit {
                try? await Task.sleep(for: .milliseconds(50))
                inputClient.simulateReturn()
            }

            onComplete?(.committed(id: id, text: finalText, submitted: submit))
        } else if insert {
            // Empty result — just deactivate
            await MainActor.run { inputClient.deactivate() }
            ime = .committed
            onComplete?(.committed(id: id, text: finalText, submitted: false))
        } else {
            // Cancel path — IME already cleared
            onComplete?(.cancelled(id: id, text: finalText))
        }
    }
}

// MARK: - Layer States

extension Session {
    enum CaptureState: Sendable {
        case buffering
        case draining
        case delivered
        case discarded
    }

    enum ASRState: Sendable {
        case idle
        case streaming
        case finalizing
        case done
    }

    enum IMEState: Sendable {
        case inactive
        case active
        case parked
        case committed
        case cleared
        case tornDown
    }
}

// MARK: - Supporting types

struct StreamingUpdate: Sendable {
    let text: String
    let committedUTF16Count: Int
    let detectedLanguage: String?
}

struct SessionDiagnostics: Sendable {
    var startedAt: Date?
    var endedAt: Date?
    var ending: String = ""
    var nativeRate: Double = 0

    // Streaming
    var streamingFeeds: Int = 0
    var streamedNativeSamples: Int = 0
    var streamedResampledSamples: Int = 0

    // Capture totals
    var totalNativeSamples: Int = 0

    // Finalization
    var remainingNativeSamples: Int = 0
    var remainingResampledSamples: Int = 0
    var finalizeStartedAt: Date?
    var finalizeEndedAt: Date?
    var finalText: String = ""
    var audioWavPath: String = ""

    var recordingDurationMs: Int {
        guard let s = startedAt, let e = endedAt else { return 0 }
        return Int((e.timeIntervalSince(s) * 1000).rounded())
    }

    var finalizeDurationMs: Int {
        guard let s = finalizeStartedAt, let e = finalizeEndedAt else { return 0 }
        return Int((e.timeIntervalSince(s) * 1000).rounded())
    }

    var totalNativeDurationMs: Int {
        guard nativeRate > 0 else { return 0 }
        return Int((Double(totalNativeSamples) / nativeRate * 1000).rounded())
    }

    var remainingNativeDurationMs: Int {
        guard nativeRate > 0 else { return 0 }
        return Int((Double(remainingNativeSamples) / nativeRate * 1000).rounded())
    }
}

enum SessionResult: Sendable {
    case aborted(id: UUID)
    case cancelled(id: UUID, text: String)
    case committed(id: UUID, text: String, submitted: Bool)
}
