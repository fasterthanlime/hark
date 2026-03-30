import AVFoundation
import Foundation

// MARK: - Audio Engine

/// Shared audio infrastructure. Runs continuously when warm, capturing audio
/// into a circular pre-buffer that sessions tap into.
///
/// Not an actor — uses NSLock for low-latency synchronization on audio threads.
final class AudioEngine: @unchecked Sendable {
    // MARK: - State

    enum State {
        case cold
        case warm
    }

    private(set) var state: State = .cold
    private let lock = NSLock()

    // Audio engine
    private var engine: AVAudioEngine?
    private let targetSampleRate: Double = 16_000
    private let targetChannelCount: AVAudioChannelCount = 1

    // Circular pre-buffer (~200ms at 16kHz)
    private let preBufferCapacity = 3200 // 200ms * 16kHz
    private var preBuffer: [Float] = []
    private var preBufferWriteIndex = 0

    // Per-session capture state
    private var activeCaptures: [UUID: CaptureState] = [:]

    var selectedDeviceUID: String?
    var deviceWarmPolicy: [String: Bool] = [:] // UID → warm

    // Device monitoring
    var onDeviceListChanged: (() -> Void)?

    // MARK: - Engine Lifecycle

    func warmUp() throws {
        lock.lock()
        defer { lock.unlock() }
        guard state == .cold else { return }

        let engine = AVAudioEngine()
        let inputNode = engine.inputNode
        let nativeFormat = inputNode.outputFormat(forBus: 0)

        let targetFormat = AVAudioFormat(
            commonFormat: .pcmFormatFloat32,
            sampleRate: targetSampleRate,
            channels: targetChannelCount,
            interleaved: false
        )!

        inputNode.installTap(onBus: 0, bufferSize: 1024, format: nativeFormat) {
            [weak self] buffer, _ in
            self?.handleAudioBuffer(buffer, nativeFormat: nativeFormat)
        }

        try engine.start()
        self.engine = engine
        self.preBuffer = [Float](repeating: 0, count: preBufferCapacity)
        self.preBufferWriteIndex = 0
        self.state = .warm
    }

    func coolDown() {
        lock.lock()
        defer { lock.unlock() }
        guard state == .warm else { return }

        engine?.inputNode.removeTap(onBus: 0)
        engine?.stop()
        engine = nil
        preBuffer = []
        state = .cold
    }

    // MARK: - Capture API (called by Session)

    func startCapture(for session: Session) {
        lock.lock()
        defer { lock.unlock() }

        var state = CaptureState()

        // Copy pre-buffer contents into the capture buffer
        if preBufferCapacity > 0 {
            let filled = min(preBuffer.count, preBufferCapacity)
            // Read from pre-buffer in order (oldest to newest)
            for i in 0..<filled {
                let readIndex = (preBufferWriteIndex + i) % preBufferCapacity
                state.samples.append(preBuffer[readIndex])
            }
        }

        activeCaptures[session.id] = state
    }

    func peekCapture(for session: Session) -> [Float] {
        lock.lock()
        defer { lock.unlock() }
        return activeCaptures[session.id]?.samples ?? []
    }

    func stopCapture(for session: Session) -> [Float] {
        lock.lock()
        defer { lock.unlock() }
        let samples = activeCaptures.removeValue(forKey: session.id)?.samples ?? []
        return samples
    }

    func cancelCapture(for session: Session) {
        lock.lock()
        defer { lock.unlock() }
        activeCaptures.removeValue(forKey: session.id)
    }

    // MARK: - Audio Callback

    private func handleAudioBuffer(_ buffer: AVAudioPCMBuffer, nativeFormat: AVAudioFormat) {
        // TODO: resample to 16kHz mono if needed
        guard let channelData = buffer.floatChannelData else { return }
        let count = Int(buffer.frameLength)
        let samples = Array(UnsafeBufferPointer(start: channelData[0], count: count))

        lock.lock()
        defer { lock.unlock() }

        // Write to circular pre-buffer
        for sample in samples {
            preBuffer[preBufferWriteIndex] = sample
            preBufferWriteIndex = (preBufferWriteIndex + 1) % preBufferCapacity
        }

        // Append to all active captures
        for id in activeCaptures.keys {
            activeCaptures[id]?.samples.append(contentsOf: samples)
            activeCaptures[id]?.lastRMS = rms(samples)
        }
    }

    private func rms(_ samples: [Float]) -> Float {
        guard !samples.isEmpty else { return 0 }
        let sum = samples.reduce(Float(0)) { $0 + $1 * $1 }
        return sqrt(sum / Float(samples.count))
    }

    // MARK: - Per-capture state

    private struct CaptureState {
        var samples: [Float] = []
        var lastRMS: Float = 0
    }
}

// MARK: - Device Management

extension AudioEngine {
    struct InputDevice: Identifiable, Sendable {
        let id: String // UID
        let name: String
        let isBuiltIn: Bool
        let isDefault: Bool
    }

    func setupDeviceMonitoring() {
        // TODO: install Core Audio property listeners for
        // kAudioHardwarePropertyDefaultInputDevice and
        // kAudioHardwarePropertyDevices
    }

    func selectDevice(uid: String) {
        selectedDeviceUID = uid
        // TODO: if warm, restart engine on new device
        // Bee MUST NOT change the system default
    }
}
