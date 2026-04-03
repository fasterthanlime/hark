import AppKit
import Carbon
import Foundation

/// Communicates with the beeInput IME via Vox IPC (Unix socket).
final class BeeInputClient: Sendable {
    private static let beeBundleID = "fasterthanlime.inputmethod.bee"

    init() {
        Task { await BeeIPCServer.shared.start() }
    }

    // MARK: - Input Source Switching

    @discardableResult
    func activate(sessionID: UUID, targetPID: pid_t?) async -> Bool {
        beeLog("IME ACTIVATE: prepareSession start id=\(sessionID.uuidString.prefix(8))")
        await BeeIPCServer.shared.prepareDictationSession(
            sessionId: sessionID.uuidString,
            targetPid: Int32(targetPID ?? 0)
        )

        beeLog("IME ACTIVATE: TIS SELECT start id=\(sessionID.uuidString.prefix(8))")
        let selected = await Self.selectBeeInputSource()
        guard selected else {
            beeLog("IME ACTIVATE: TIS SELECT failed id=\(sessionID.uuidString.prefix(8))")
            return false
        }

        beeLog("IME ACTIVATE: done id=\(sessionID.uuidString.prefix(8)), waiting for imeAttach")
        return true
    }

    @MainActor
    private static func selectBeeInputSource() async -> Bool {
        guard let beeSource = findBeeInputSource() else {
            beeLog("IME ACTIVATE: bee input source NOT FOUND")
            return false
        }
        let result = TISSelectInputSource(beeSource)
        beeLog("TIS SELECT: \(inputSourceID(beeSource)) result=\(result)")
        return result == noErr
    }

    func deactivate(caller: String = #function, file: String = #fileID, line: Int = #line) {
        beeLog("IME DEACTIVATE called from \(file):\(line) \(caller)")
        if let source = Self.findBeeInputSource() {
            let result = TISDeselectInputSource(source)
            beeLog("TIS DESELECT: \(Self.inputSourceID(source)) result=\(result)")
        }
    }

    static func waitForIMEReady() async -> Bool {
        await BeeIPCServer.shared.waitForIMEReady()
    }

    // MARK: - IME Commands

    func setMarkedText(_ text: String, sessionID: UUID) {
        beeLog("setMarkedText → vox session=\(sessionID.uuidString.prefix(8))")
        Task { await BeeIPCServer.shared.setMarkedText(sessionId: sessionID.uuidString, text: text) }
    }

    func logSetMarkedText(_ text: String, sessionID: UUID) {
        beeLog("IME setMarkedText: \(text.prefix(60).debugDescription)")
        setMarkedText(text, sessionID: sessionID)
    }

    func commitText(_ text: String, sessionID: UUID) {
        Task { await BeeIPCServer.shared.commitText(sessionId: sessionID.uuidString, text: text) }
    }

    func clearMarkedText(sessionID: UUID) {
        Task { await BeeIPCServer.shared.stopDictating(sessionId: sessionID.uuidString) }
    }

    func stopDictating(sessionID: UUID) {
        Task { await BeeIPCServer.shared.stopDictating(sessionId: sessionID.uuidString) }
    }

    func simulateReturn() {
        let src = CGEventSource(stateID: .hidSystemState)
        if let down = CGEvent(keyboardEventSource: src, virtualKey: 0x24, keyDown: true),
            let up = CGEvent(keyboardEventSource: src, virtualKey: 0x24, keyDown: false)
        {
            down.post(tap: .cghidEventTap)
            usleep(10_000)
            up.post(tap: .cghidEventTap)
        }
    }

    // MARK: - IME Registration

    private static func bundleVersion(_ url: URL) -> String? {
        guard let bundle = Bundle(url: url) else { return nil }
        return bundle.infoDictionary?["CFBundleVersion"] as? String
    }

    private static func installViaAppleScriptTask(src: String, dst: String, parent: String) async -> Bool {
        guard let scriptsDir = FileManager.default.urls(
            for: .applicationScriptsDirectory, in: .userDomainMask).first
        else {
            beeLog("IME REGISTER: no application scripts directory")
            return false
        }
        try? FileManager.default.createDirectory(at: scriptsDir, withIntermediateDirectories: true)
        let scriptURL = scriptsDir.appendingPathComponent("install-beeInput.applescript")
        let scriptSrc = "do shell script \"rm -rf '\(dst)' && cp -r '\(src)' '\(parent)/'\" with administrator privileges"
        do {
            try scriptSrc.write(to: scriptURL, atomically: true, encoding: .utf8)
            let task = try NSUserAppleScriptTask(url: scriptURL)
            return await withCheckedContinuation { continuation in
                task.execute(withAppleEvent: nil, completionHandler: { _, error in
                    if let error {
                        beeLog("IME REGISTER: script task failed: \(error)")
                        continuation.resume(returning: false)
                    } else {
                        beeLog("IME REGISTER: installed beeInput.app via NSUserAppleScriptTask")
                        continuation.resume(returning: true)
                    }
                })
            }
        } catch {
            beeLog("IME REGISTER: script setup failed: \(error)")
            return false
        }
    }

    @discardableResult
    static func ensureIMERegistered() async -> Bool {
        let allProps: [CFString: Any] = [kTISPropertyBundleID: beeBundleID as CFString]

        let bundledIME = Bundle.main.bundleURL
            .appendingPathComponent("Contents/Library/Input Methods/beeInput.app")
        let installedIME = FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent("Library/Input Methods/beeInput.app")

        guard FileManager.default.fileExists(atPath: bundledIME.path) else {
            beeLog("IME REGISTER: beeInput.app not found in bundle")
            return false
        }

        let installedVersion = bundleVersion(installedIME)
        let bundledVersion = bundleVersion(bundledIME)
        let needsInstall = installedVersion != bundledVersion

        beeLog("IME REGISTER: installed=\(installedVersion ?? "none") bundled=\(bundledVersion ?? "?") needsInstall=\(needsInstall)")

        if needsInstall {
            let confirmed = await MainActor.run {
                let alert = NSAlert()
                alert.messageText = "Install bee Input Method"
                alert.informativeText =
                    "bee needs to install its input method component to enable dictation. This only happens once per update."
                alert.addButton(withTitle: "Install")
                alert.addButton(withTitle: "Later")
                return alert.runModal() == .alertFirstButtonReturn
            }
            guard confirmed else {
                beeLog("IME REGISTER: user deferred install")
                return false
            }

            let src = bundledIME.path
            let dst = installedIME.path
            let parent = installedIME.deletingLastPathComponent().path
            guard await installViaAppleScriptTask(src: src, dst: dst, parent: parent) else {
                return false
            }
        }

        let allSources =
            (TISCreateInputSourceList(allProps as CFDictionary, true)?
                .takeRetainedValue() as? [TISInputSource]) ?? []
        beeLog("IME REGISTER: found \(allSources.count) source(s) (includeAll=true)")

        if let source = allSources.first {
            let enabled =
                TISGetInputSourceProperty(source, kTISPropertyInputSourceIsEnabled)
                .map { Unmanaged<CFNumber>.fromOpaque($0).takeUnretainedValue() as! Bool } ?? false
            if !enabled {
                beeLog("IME REGISTER: source disabled, enabling")
                TISEnableInputSource(source)
            }
            return true
        }

        guard FileManager.default.fileExists(atPath: installedIME.path) else {
            beeLog("IME REGISTER: beeInput.app not available for registration")
            return false
        }

        let status = TISRegisterInputSource(installedIME as CFURL)
        beeLog("IME REGISTER: TISRegisterInputSource result=\(status)")
        guard status == noErr else { return false }

        let newSources =
            (TISCreateInputSourceList(allProps as CFDictionary, true)?
                .takeRetainedValue() as? [TISInputSource]) ?? []
        beeLog("IME REGISTER: after registration, found \(newSources.count) source(s)")
        if let source = newSources.first {
            TISEnableInputSource(source)
            return true
        }
        return false
    }

    static func restoreInputSourceIfNeeded(
        caller: String = #function, file: String = #fileID, line: Int = #line
    ) {
        // Palette input sources stay selected permanently — nothing to restore.
    }

    private static func inputSourceID(_ source: TISInputSource) -> String {
        guard let raw = TISGetInputSourceProperty(source, kTISPropertyInputSourceID) else {
            return "<unknown>"
        }
        return Unmanaged<CFString>.fromOpaque(raw).takeUnretainedValue() as String
    }

    private static func findBeeInputSource() -> TISInputSource? {
        let properties: [CFString: Any] = [kTISPropertyBundleID: beeBundleID as CFString]
        guard
            let sources = TISCreateInputSourceList(properties as CFDictionary, false)?
                .takeRetainedValue() as? [TISInputSource],
            let source = sources.first
        else { return nil }
        return source
    }
}
