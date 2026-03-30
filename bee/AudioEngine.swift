import AVFoundation
import Foundation
import os

private let logger = Logger(subsystem: "fasterthanlime.bee", category: "AudioEngine")

/// Shared audio infrastructure. Runs continuously when warm, capturing audio
/// into a circular pre-buffer that sessions tap into.
///
/// Stores samples at the device's native sample rate internally. Resampling
/// to 16kHz happens when samples are read out (peekCapture/stopCapture).
final class AudioEngine: @unchecked Sendable {
    enum State {
        case cold
        case warm
    }

    private(set) var state: State = .cold
    private let lock = NSLock()

    private var engine: AVAudioEngine?
    private var nativeSampleRate: Double = 0
    private let targetSampleRate: Double = 16_000

    // Circular pre-buffer for warm mode (~200ms at native rate)
    private let preBufferDuration: TimeInterval = 0.2
    private var preBuffer: [Float] = []
    private var preBufferWriteIndex = 0
    private var preBufferCapacity = 0

    // Per-session capture state
    private var activeCaptures: [UUID: CaptureHandle] = [:]

    // VAD parameters
    private let vadFastWaitSeconds: TimeInterval = 0.12
    private let vadSpeechWaitSeconds: TimeInterval = 0.28
    private let vadRequiredSilenceSeconds: TimeInterval = 0.05
    private let vadSpeechRmsThreshold: Float = 0.012
    private let vadSilenceRmsThreshold: Float = 0.008
    private let vadBoundaryTimeoutSeconds: TimeInterval = 0.55

    // Device management
    var selectedDeviceUID: String?
    var deviceWarmPolicy: [String: Bool] = [:]
    var onDeviceListChanged: (() -> Void)?

    // MARK: - Engine Lifecycle

    func warmUp() throws {
        lock.lock()
        guard state == .cold else { lock.unlock(); return }
        lock.unlock()

        let engine = AVAudioEngine()
        let inputNode = engine.inputNode
        let nativeFormat = inputNode.outputFormat(forBus: 0)

        guard nativeFormat.sampleRate > 0 else {
            throw AudioEngineError.noMicrophone
        }

        let nativeRate = nativeFormat.sampleRate

        lock.lock()
        self.nativeSampleRate = nativeRate
        preBufferCapacity = Int(nativeRate * preBufferDuration)
        preBuffer = [Float](repeating: 0, count: preBufferCapacity)
        preBufferWriteIndex = 0
        state = .warm
        lock.unlock()

        inputNode.installTap(onBus: 0, bufferSize: 1024, format: nativeFormat) {
            [weak self] buffer, _ in
            self?.handleAudioBuffer(buffer)
        }

        try engine.start()
        self.engine = engine
        logger.info("Audio engine warm: native rate = \(nativeRate) Hz")
    }

    func coolDown() {
        lock.lock()
        state = .cold
        preBuffer.removeAll()
        preBufferCapacity = 0
        preBufferWriteIndex = 0
        lock.unlock()

        engine?.inputNode.removeTap(onBus: 0)
        engine?.stop()
        engine = nil
        logger.info("Audio engine cooled down")
    }

    var isWarm: Bool {
        lock.withLock { state == .warm }
    }

    // MARK: - Capture API

    /// Begin capturing audio for a session. Copies the pre-buffer.
    func startCapture(for sessionID: UUID) {
        lock.lock()
        defer { lock.unlock() }

        var handle = CaptureHandle()

        // Copy pre-buffer (circular read)
        if preBufferCapacity > 0 {
            if preBufferWriteIndex >= preBufferCapacity {
                let startIndex = preBufferWriteIndex % preBufferCapacity
                handle.samples.append(contentsOf: preBuffer[startIndex...])
                handle.samples.append(contentsOf: preBuffer[..<startIndex])
            } else {
                handle.samples.append(contentsOf: preBuffer[..<preBufferWriteIndex])
            }
        }

        handle.isCapturing = true
        activeCaptures[sessionID] = handle
    }

    /// Peek at current captured audio, resampled to 16kHz.
    func peekCapture(for sessionID: UUID) -> [Float] {
        lock.lock()
        let samples = activeCaptures[sessionID]?.samples ?? []
        let nativeRate = nativeSampleRate
        lock.unlock()

        guard !samples.isEmpty else { return [] }
        if nativeRate == targetSampleRate { return samples }
        return resample(samples, from: nativeRate, to: targetSampleRate)
    }

    /// Stop capturing with VAD tail drain. Blocks until silence or timeout.
    /// Returns samples resampled to 16kHz.
    func stopCapture(for sessionID: UUID) -> [Float] {
        let signal = DispatchSemaphore(value: 0)

        lock.lock()
        guard var handle = activeCaptures[sessionID], handle.isCapturing else {
            let samples = activeCaptures.removeValue(forKey: sessionID)?.samples ?? []
            let nativeRate = nativeSampleRate
            lock.unlock()
            if nativeRate == targetSampleRate { return samples }
            return samples.isEmpty ? [] : resample(samples, from: nativeRate, to: targetSampleRate)
        }

        let lastRms = handle.lastRms
        let maxWait = lastRms >= vadSpeechRmsThreshold ? vadSpeechWaitSeconds : vadFastWaitSeconds

        handle.isCapturing = false
        handle.isDraining = true
        handle.drainSignal = signal
        handle.drainSilenceSamples = 0
        handle.drainSamplesUntilTimeout = max(1, Int((nativeSampleRate * maxWait).rounded()))
        activeCaptures[sessionID] = handle
        lock.unlock()

        // Block until VAD says we're done
        let waitResult = signal.wait(timeout: .now() + vadBoundaryTimeoutSeconds)

        lock.lock()
        if waitResult == .timedOut {
            if var h = activeCaptures[sessionID] {
                h.isDraining = false
                activeCaptures[sessionID] = h
            }
        }
        let samples = activeCaptures.removeValue(forKey: sessionID)?.samples ?? []
        let nativeRate = nativeSampleRate
        lock.unlock()

        logger.info("stopCapture: \(samples.count) native samples, timeout=\(waitResult == .timedOut)")

        guard !samples.isEmpty else { return [] }
        if nativeRate == targetSampleRate { return samples }
        return resample(samples, from: nativeRate, to: targetSampleRate)
    }

    /// Cancel capture, discard audio. Non-blocking.
    func cancelCapture(for sessionID: UUID) {
        lock.lock()
        activeCaptures.removeValue(forKey: sessionID)
        lock.unlock()
    }

    // MARK: - Audio Callback

    private func handleAudioBuffer(_ buffer: AVAudioPCMBuffer) {
        guard let channelData = buffer.floatChannelData else { return }
        let count = Int(buffer.frameLength)
        guard count > 0 else { return }

        let bufferPointer = UnsafeBufferPointer(start: channelData[0], count: count)
        let rms = computeRMS(bufferPointer)

        var signalsToFire: [DispatchSemaphore] = []

        lock.lock()

        // Update active captures
        for (id, var handle) in activeCaptures {
            if handle.isCapturing || handle.isDraining {
                handle.samples.append(contentsOf: bufferPointer)
                handle.lastRms = rms

                if handle.isDraining {
                    if rms < vadSilenceRmsThreshold {
                        handle.drainSilenceSamples += count
                    } else {
                        handle.drainSilenceSamples = 0
                    }
                    handle.drainSamplesUntilTimeout = max(0, handle.drainSamplesUntilTimeout - count)

                    let requiredSilence = max(1, Int((nativeSampleRate * vadRequiredSilenceSeconds).rounded()))
                    let reachedSilence = handle.drainSilenceSamples >= requiredSilence
                    let reachedTimeout = handle.drainSamplesUntilTimeout == 0

                    if reachedSilence || reachedTimeout {
                        handle.isDraining = false
                        if let sig = handle.drainSignal {
                            signalsToFire.append(sig)
                            handle.drainSignal = nil
                        }
                    }
                }

                activeCaptures[id] = handle
            }
        }

        // Fill circular pre-buffer (when no capture is active, or always)
        if state == .warm && preBufferCapacity > 0 {
            for sample in bufferPointer {
                preBuffer[preBufferWriteIndex % preBufferCapacity] = sample
                preBufferWriteIndex += 1
            }
        }

        lock.unlock()

        for sig in signalsToFire {
            sig.signal()
        }
    }

    private func computeRMS(_ samples: UnsafeBufferPointer<Float>) -> Float {
        guard !samples.isEmpty else { return 0 }
        var sum: Float = 0
        for s in samples { sum += s * s }
        return sqrtf(sum / Float(samples.count))
    }

    // MARK: - Resampling

    private func resample(_ samples: [Float], from srcRate: Double, to dstRate: Double) -> [Float] {
        guard let srcFormat = AVAudioFormat(
            commonFormat: .pcmFormatFloat32, sampleRate: srcRate, channels: 1, interleaved: false
        ), let dstFormat = AVAudioFormat(
            commonFormat: .pcmFormatFloat32, sampleRate: dstRate, channels: 1, interleaved: false
        ), let converter = AVAudioConverter(from: srcFormat, to: dstFormat) else {
            return samples
        }

        let srcCount = AVAudioFrameCount(samples.count)
        guard let srcBuffer = AVAudioPCMBuffer(pcmFormat: srcFormat, frameCapacity: srcCount) else {
            return samples
        }
        srcBuffer.frameLength = srcCount
        if let cd = srcBuffer.floatChannelData {
            samples.withUnsafeBufferPointer { cd[0].update(from: $0.baseAddress!, count: samples.count) }
        }

        let dstCount = AVAudioFrameCount(Double(srcCount) * dstRate / srcRate) + 1
        guard let dstBuffer = AVAudioPCMBuffer(pcmFormat: dstFormat, frameCapacity: dstCount) else {
            return samples
        }

        var consumed = false
        _ = converter.convert(to: dstBuffer, error: nil) { _, outStatus in
            if consumed { outStatus.pointee = .endOfStream; return nil }
            consumed = true
            outStatus.pointee = .haveData
            return srcBuffer
        }

        guard let cd = dstBuffer.floatChannelData else { return samples }
        return Array(UnsafeBufferPointer(start: cd[0], count: Int(dstBuffer.frameLength)))
    }

    // MARK: - Mic Permission

    static func requestPermission() async -> Bool {
        let status = AVCaptureDevice.authorizationStatus(for: .audio)
        switch status {
        case .authorized: return true
        case .notDetermined: return await AVCaptureDevice.requestAccess(for: .audio)
        default: return false
        }
    }

    func selectDevice(uid: String) {
        selectedDeviceUID = uid
    }

    // MARK: - Per-capture state

    private struct CaptureHandle {
        var samples: [Float] = []
        var isCapturing = false
        var isDraining = false
        var lastRms: Float = 0
        var drainSignal: DispatchSemaphore?
        var drainSilenceSamples = 0
        var drainSamplesUntilTimeout = 0
    }
}

enum AudioEngineError: LocalizedError {
    case noMicrophone
    var errorDescription: String? { "No microphone available" }
}
