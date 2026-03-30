import AppKit
import CoreAudio
import os

/// Pauses/resumes system media playback during dictation.
@MainActor
struct MediaController {
    private static let logger = Logger(
        subsystem: Bundle.main.bundleIdentifier ?? "hark",
        category: "MediaController"
    )
    private static var didPauseMedia = false

    /// Pause system media if any process (other than ourselves / system services) is driving audio output.
    static func pauseIfPlaying() {
        didPauseMedia = false

        let myPID = ProcessInfo.processInfo.processIdentifier
        let (active, bundle, pid) = findActiveAudioOutput(excludingPID: myPID)
        logger.warning("[media] Audio output active: \(active) pid=\(pid) bundle=\(bundle ?? "?", privacy: .public)")

        guard active else { return }
        mediaRemoteSendCommand(1) // kMRPause
        didPauseMedia = true
        logger.warning("[media] Sent pause command")
    }

    /// If we paused media earlier, resume it.
    static func resumeIfPaused() {
        guard didPauseMedia else { return }
        mediaRemoteSendCommand(0) // kMRPlay
        didPauseMedia = false
        logger.warning("[media] Sent play command")
    }

    /// Setting stored in UserDefaults.
    static var isEnabled: Bool {
        get { UserDefaults.standard.bool(forKey: "pauseMediaWhileDictating") }
        set { UserDefaults.standard.set(newValue, forKey: "pauseMediaWhileDictating") }
    }

    // MARK: - Core Audio Process Detection

    /// Bundles to ignore when checking for active audio output.
    private static let ignoredBundles: Set<String> = [
        "com.apple.CoreSpeech",
    ]

    /// Check if any process (excluding the given PID and ignored bundles) is currently producing audio output.
    private static func findActiveAudioOutput(excludingPID: pid_t) -> (active: Bool, bundle: String?, pid: pid_t) {
        for obj in getProcessObjects() {
            let pid = getProcessPID(obj)
            guard pid != excludingPID else { continue }
            let bundle = getProcessBundleID(obj)
            if let bundle, ignoredBundles.contains(bundle) { continue }
            if isProcessRunningOutput(obj) {
                logger.warning("[media] Found active audio output: pid=\(pid) bundle=\(bundle ?? "?", privacy: .public)")
                return (true, bundle, pid)
            }
        }
        return (false, nil, 0)
    }

    private static func getProcessObjects() -> [AudioObjectID] {
        var address = AudioObjectPropertyAddress(
            mSelector: kAudioHardwarePropertyProcessObjectList,
            mScope: kAudioObjectPropertyScopeGlobal,
            mElement: kAudioObjectPropertyElementMain
        )
        var size: UInt32 = 0
        guard AudioObjectGetPropertyDataSize(
            AudioObjectID(kAudioObjectSystemObject), &address, 0, nil, &size
        ) == noErr else { return [] }

        let count = Int(size) / MemoryLayout<AudioObjectID>.size
        var objects = [AudioObjectID](repeating: 0, count: count)
        guard AudioObjectGetPropertyData(
            AudioObjectID(kAudioObjectSystemObject), &address, 0, nil, &size, &objects
        ) == noErr else { return [] }
        return objects
    }

    private static func getProcessPID(_ obj: AudioObjectID) -> pid_t {
        var address = AudioObjectPropertyAddress(
            mSelector: kAudioProcessPropertyPID,
            mScope: kAudioObjectPropertyScopeGlobal,
            mElement: kAudioObjectPropertyElementMain
        )
        var pid: pid_t = 0
        var size = UInt32(MemoryLayout<pid_t>.size)
        AudioObjectGetPropertyData(obj, &address, 0, nil, &size, &pid)
        return pid
    }

    private static func getProcessBundleID(_ obj: AudioObjectID) -> String? {
        var address = AudioObjectPropertyAddress(
            mSelector: kAudioProcessPropertyBundleID,
            mScope: kAudioObjectPropertyScopeGlobal,
            mElement: kAudioObjectPropertyElementMain
        )
        var cfStr: Unmanaged<CFString>?
        var size = UInt32(MemoryLayout<Unmanaged<CFString>?>.size)
        guard AudioObjectGetPropertyData(obj, &address, 0, nil, &size, &cfStr) == noErr,
              let str = cfStr?.takeUnretainedValue() else { return nil }
        return str as String
    }

    private static func isProcessRunningOutput(_ obj: AudioObjectID) -> Bool {
        var address = AudioObjectPropertyAddress(
            mSelector: kAudioProcessPropertyIsRunningOutput,
            mScope: kAudioObjectPropertyScopeGlobal,
            mElement: kAudioObjectPropertyElementMain
        )
        var running: UInt32 = 0
        var size = UInt32(MemoryLayout<UInt32>.size)
        AudioObjectGetPropertyData(obj, &address, 0, nil, &size, &running)
        return running != 0
    }

    // MARK: - MediaRemote (for sending pause/play commands)

    private static let mrHandle: UnsafeMutableRawPointer? = {
        dlopen("/System/Library/PrivateFrameworks/MediaRemote.framework/MediaRemote", RTLD_LAZY)
    }()

    private static func mediaRemoteSendCommand(_ command: UInt32) {
        guard let handle = mrHandle,
              let sym = dlsym(handle, "MRMediaRemoteSendCommand") else {
            logger.warning("[media] sendCommand: no handle or sym")
            return
        }
        typealias Fn = @convention(c) (UInt32, UnsafeRawPointer?) -> Bool
        let ok = unsafeBitCast(sym, to: Fn.self)(command, nil)
        logger.warning("[media] sendCommand(\(command)) returned \(ok)")
    }
}
