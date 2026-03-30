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

    private var processedSampleCount: Int = 0
    private var asrSession: StreamingSession?
    private var streamingTask: Task<Void, Never>?
    private var finalText: String = ""
    private var partialTranscript: String = ""

    private var onStreamingUpdate: (@Sendable (String) -> Void)?
    private var onComplete: (@Sendable (SessionResult) -> Void)?

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
        audioEngine.startCapture(for: self.id)

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
        ime = .tornDown

        onComplete?(.aborted(id: id))
    }

    /// Finalize in background, create history entry, but don't insert text.
    func cancel() async {
        guard !isTerminal else { return }
        logger.info("[\(self.id)] Cancelling")

        streamingTask?.cancel()

        capture = .draining
        let samples = audioEngine.stopCapture(for: self.id)
        capture = .delivered

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

        capture = .draining
        let samples = audioEngine.stopCapture(for: self.id)
        capture = .delivered

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

        while !Task.isCancelled && capture == .buffering {
            let allSamples = audioEngine.peekCapture(for: self.id)
            let newCount = allSamples.count

            // Need at least 800 new samples (~50ms at 16kHz) before feeding
            guard newCount > processedSampleCount + 800 else {
                try? await Task.sleep(for: .milliseconds(30))
                continue
            }

            let chunk = Array(allSamples[processedSampleCount...])
            processedSampleCount = newCount

            if let update = transcriptionService.feed(session: session, samples: chunk) {
                partialTranscript = update.text
                onStreamingUpdate?(update.text)
            }
        }
    }

    private func finalize(samples: [Float], insert: Bool, submit: Bool) async {
        asr = .finalizing

        guard let session = asrSession else {
            asr = .done
            finalText = partialTranscript
            completeSession(insert: insert, submit: submit)
            return
        }

        // Feed remaining unprocessed samples
        let remaining = samples.count > processedSampleCount
            ? Array(samples[processedSampleCount...])
            : []

        if !remaining.isEmpty {
            // Add silence padding for trailing speech
            var finalChunk = remaining
            let padSamples = Int(16_000 * 0.1) // 100ms padding
            finalChunk.append(contentsOf: repeatElement(Float(0), count: padSamples))

            if let update = transcriptionService.feedFinalizing(session: session, samples: finalChunk) {
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

        logger.info("[\(self.id)] Finalized: \"\(self.finalText.prefix(80))\"")
        completeSession(insert: insert, submit: submit)
    }

    private func completeSession(insert: Bool, submit: Bool) {
        if insert {
            onComplete?(.committed(id: id, text: finalText, submitted: submit))
        } else {
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

enum SessionResult: Sendable {
    case aborted(id: UUID)
    case cancelled(id: UUID, text: String)
    case committed(id: UUID, text: String, submitted: Bool)
}
