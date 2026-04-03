import AppKit
import Carbon
import Foundation

private final class BeeAppControlSink: NSObject, BeeBrokerPeerXPC {
    private static let imeSubmitName = NSNotification.Name("fasterthanlime.bee.imeSubmit")
    private static let imeCancelName = NSNotification.Name("fasterthanlime.bee.imeCancel")
    private static let imeUserTypedName = NSNotification.Name("fasterthanlime.bee.imeUserTyped")
    private static let imeContextLostName = NSNotification.Name("fasterthanlime.bee.imeContextLost")
    private static let imeSessionStartedName = NSNotification.Name(
        "fasterthanlime.bee.imeSessionStarted")

    private func post(_ name: NSNotification.Name, userInfo: [AnyHashable: Any]) {
        NotificationCenter.default.post(name: name, object: nil, userInfo: userInfo)
    }

    func handleIMESessionStarted(_ sessionID: String) {
        post(Self.imeSessionStartedName, userInfo: ["sessionID": sessionID])
    }

    func handleIMESubmit(_ sessionID: String) {
        post(Self.imeSubmitName, userInfo: ["sessionID": sessionID])
    }

    func handleIMECancel(_ sessionID: String) {
        post(Self.imeCancelName, userInfo: ["sessionID": sessionID])
    }

    func handleIMEUserTyped(_ sessionID: String, keyCode: Int32, characters: String) {
        post(
            Self.imeUserTypedName,
            userInfo: [
                "sessionID": sessionID,
                "keyCode": Int(keyCode),
                "characters": characters,
            ]
        )
    }

    func handleIMEContextLost(_ sessionID: String, hadMarkedText: Bool) {
        post(
            Self.imeContextLostName,
            userInfo: [
                "sessionID": sessionID,
                "hadMarkedText": hadMarkedText,
            ]
        )
    }

    func handleNewPreparedSession(_ sessionID: String, targetPID: Int32) {}
    func handleClearSession(_ sessionID: String) {}
    func handleSetMarkedText(_ sessionID: String, text: String) {}
    func handleCommitText(_ sessionID: String, text: String, submit: Bool) {}
    func handleCancelInput(_ sessionID: String) {}
    func handleStopDictating(_ sessionID: String) {}
}

/// Communicates with the helper broker process via XPC.
final class BeeInputClient: Sendable {
    private static let brokerServiceName = "fasterthanlime.bee.broker"
    private static let brokerLaunchLabel = "fasterthanlime.bee.broker"
    private static let beeBundleID = "fasterthanlime.inputmethod.bee"
    private static let appInstanceID = UUID().uuidString

    nonisolated(unsafe) private static var xpcConnection: NSXPCConnection?
    nonisolated(unsafe) private static var appControlSink = BeeAppControlSink()
    nonisolated(unsafe) private static var helloSent = false
    nonisolated(unsafe) private static var brokerLaunchAttempted = false
    private static let xpcLock = NSLock()

    init() {
        Self.ensureBrokerLaunchdService()
        Self.sendHelloIfNeeded()
    }

    // MARK: - Input Source Switching

    @discardableResult
    func activate(sessionID: UUID, targetPID: pid_t?) async -> Bool {
        let activationID = UUID().uuidString
        beeLog("IME ACTIVATE: prepareSessionXPC start id=\(sessionID.uuidString.prefix(8))")
        let prepared = await Self.prepareSessionXPC(
            sessionID: sessionID,
            activationID: activationID,
            targetPID: Int32(targetPID ?? 0)
        )
        guard prepared else {
            beeLog(
                "IME ACTIVATE: prepareSession failed for session=\(sessionID.uuidString.prefix(8))")
            return false
        }
        beeLog("IME ACTIVATE: prepareSessionXPC done id=\(sessionID.uuidString.prefix(8))")

        // Select the bee input source (on cold start this also launches the
        // IME process), then wait for the IME to be connected to the broker.
        beeLog("IME ACTIVATE: TIS SELECT start id=\(sessionID.uuidString.prefix(8))")
        let selected = await Self.selectBeeInputSource()
        guard selected else {
            await Self.clearSessionXPC(sessionID: sessionID)
            return false
        }

        beeLog(
            "IME ACTIVATE: selection done id=\(sessionID.uuidString.prefix(8)) activationID=\(activationID.prefix(8)), waiting for IME confirm event"
        )
        return true
    }

    @MainActor
    private static func selectBeeInputSource() async -> Bool {
        guard let beeSource = findBeeInputSource() else {
            beeLog("IME ACTIVATE: bee input source NOT FOUND")
            return false
        }

        let result = TISSelectInputSource(beeSource)
        beeLog("TIS SELECT: \(inputSourceID(beeSource)) (activate) result=\(result)")
        return result == noErr
    }

    func deactivate(caller: String = #function, file: String = #fileID, line: Int = #line) {
        beeLog("IME DEACTIVATE called from \(file):\(line) \(caller)")
        // Deselect the palette so the next TISSelectInputSource triggers activateServer.
        if let source = Self.findBeeInputSource() {
            let result = TISDeselectInputSource(source)
            beeLog("TIS DESELECT: \(Self.inputSourceID(source)) result=\(result)")
        }
    }

    /// Wait for the IME to connect to the broker.
    static func waitForIMEReady() async -> Bool {
        await waitForIMEXPC()
    }

    // MARK: - IME Commands

    func setMarkedText(_ text: String, sessionID: UUID) {
        beeLog("setMarkedText → broker session=\(sessionID.uuidString.prefix(8))")
        Self.setMarkedTextXPC(text, sessionID: sessionID)
    }

    func logSetMarkedText(_ text: String, sessionID: UUID) {
        beeLog("IME setMarkedText: \(text.prefix(60).debugDescription)")
        setMarkedText(text, sessionID: sessionID)
    }

    func commitText(_ text: String, sessionID: UUID) {
        Self.commitTextXPC(text, submit: false, sessionID: sessionID)
    }

    func clearMarkedText(sessionID: UUID) {
        Self.cancelInputXPC(sessionID: sessionID)
    }

    func stopDictating(sessionID: UUID) {
        Self.stopDictatingXPC(sessionID: sessionID)
    }

    func simulateReturn() {
        let src = CGEventSource(stateID: .hidSystemState)
        if let down = CGEvent(keyboardEventSource: src, virtualKey: 0x24, keyDown: true),
            let up = CGEvent(keyboardEventSource: src, virtualKey: 0x24, keyDown: false)
        {
            down.post(tap: .cghidEventTap)
            usleep(10_000)  // 10ms
            up.post(tap: .cghidEventTap)
        }
    }

    // MARK: - IME Registration

    @discardableResult
    static func ensureIMERegistered() -> Bool {
        let allProps: [CFString: Any] = [kTISPropertyBundleID: beeBundleID as CFString]
        let allSources = (TISCreateInputSourceList(allProps as CFDictionary, true)?
            .takeRetainedValue() as? [TISInputSource]) ?? []

        beeLog("IME REGISTER: found \(allSources.count) source(s) (includeAll=true)")

        if let source = allSources.first {
            let enabled = TISGetInputSourceProperty(source, kTISPropertyInputSourceIsEnabled)
                .map { Unmanaged<CFNumber>.fromOpaque($0).takeUnretainedValue() as! Bool } ?? false
            if !enabled {
                beeLog("IME REGISTER: source disabled, enabling")
                TISEnableInputSource(source)
            }
            return true
        }

        // Not registered at all — register from disk
        let inputMethodsDir = FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent("Library/Input Methods/beeInput.app")
        guard FileManager.default.fileExists(atPath: inputMethodsDir.path) else {
            beeLog("IME REGISTER: beeInput.app not found in ~/Library/Input Methods/")
            return false
        }

        let status = TISRegisterInputSource(inputMethodsDir as CFURL)
        beeLog("IME REGISTER: TISRegisterInputSource result=\(status)")
        guard status == noErr else { return false }

        // Enable the newly registered source (don't select — first hotkey will do that)
        let newSources = (TISCreateInputSourceList(allProps as CFDictionary, true)?
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

    private static func getXPCConnection() -> NSXPCConnection {
        return xpcLock.withLock {
            if let connection = xpcConnection {
                return connection
            }
            let connection = NSXPCConnection(machServiceName: brokerServiceName, options: [])
            connection.remoteObjectInterface = NSXPCInterface(with: BeeBrokerXPC.self)
            connection.exportedInterface = NSXPCInterface(with: BeeBrokerPeerXPC.self)
            connection.exportedObject = appControlSink
            connection.resume()
            xpcConnection = connection
            return connection
        }
    }

    private static func invalidateXPCConnection() {
        xpcLock.withLock {
            xpcConnection?.invalidate()
            xpcConnection = nil
            helloSent = false
        }
    }

    private static func ensureBrokerLaunchdService() {
        let shouldAttempt = xpcLock.withLock { () -> Bool in
            if brokerLaunchAttempted {
                return false
            }
            brokerLaunchAttempted = true
            return true
        }
        guard shouldAttempt else { return }

        let uid = getuid()
        let domain = "gui/\(uid)"

        // // First try to kickstart an already-bootstrapped service.
        // let kickStatus = runLaunchctl(args: ["kickstart", "-k", service])
        // if kickStatus == 0 {
        //     beeLog("BROKER launchd: kickstart ok service=\(service)")
        //     return
        // }

        // // If not bootstrapped yet, bootstrap from the per-user LaunchAgent plist.
        // let plistPath = NSHomeDirectory() + "/Library/LaunchAgents/\(brokerLaunchLabel).plist"
        // if FileManager.default.fileExists(atPath: plistPath) {
        //     _ = runLaunchctl(args: ["bootstrap", domain, plistPath])
        //     let retryStatus = runLaunchctl(args: ["kickstart", "-k", service])
        //     if retryStatus == 0 {
        //         beeLog("BROKER launchd: bootstrap+kickstart ok service=\(service)")
        //         return
        //     }
        // }

        // beeLog("BROKER launchd: unable to start service=\(service)")
    }

    @discardableResult
    private static func runLaunchctl(args: [String]) -> Int32 {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/bin/launchctl")
        process.arguments = args
        do {
            try process.run()
            process.waitUntilExit()
            return process.terminationStatus
        } catch {
            beeLog(
                "BROKER launchd: launchctl failed args=\(args.joined(separator: " ")) error=\(error.localizedDescription)"
            )
            return -1
        }
    }

    private static func prepareSessionXPC(sessionID: UUID, activationID: String, targetPID: Int32)
        async -> Bool
    {
        sendHelloIfNeeded()
        let connection = getXPCConnection()
        return await withCheckedContinuation { continuation in
            let proxy =
                connection.remoteObjectProxyWithErrorHandler { error in
                    beeLog("BROKER XPC prepareSession error: \(error.localizedDescription)")
                    invalidateXPCConnection()
                    continuation.resume(returning: false)
                } as? BeeBrokerXPC

            guard let proxy else {
                continuation.resume(returning: false)
                return
            }

            proxy.prepareSession(
                sessionID.uuidString,
                activationID: activationID,
                targetPID: targetPID,
                appInstanceID: appInstanceID
            ) { ok in
                continuation.resume(returning: ok)
            }
        }
    }

    private static func waitForIMEXPC() async -> Bool {
        let connection = getXPCConnection()
        return await withCheckedContinuation { continuation in
            let proxy =
                connection.remoteObjectProxyWithErrorHandler { error in
                    beeLog("BROKER XPC waitForIME error: \(error.localizedDescription)")
                    invalidateXPCConnection()
                    continuation.resume(returning: false)
                } as? BeeBrokerXPC

            guard let proxy else {
                continuation.resume(returning: false)
                return
            }

            proxy.waitForIME(appInstanceID: appInstanceID) { ok in
                continuation.resume(returning: ok)
            }
        }
    }

    private static func clearSessionXPC(sessionID: UUID) async {
        let connection = getXPCConnection()
        await withCheckedContinuation { continuation in
            let proxy =
                connection.remoteObjectProxyWithErrorHandler { error in
                    beeLog("BROKER XPC clearSession error: \(error.localizedDescription)")
                    invalidateXPCConnection()
                    continuation.resume()
                } as? BeeBrokerXPC

            guard let proxy else {
                continuation.resume()
                return
            }

            proxy.clearSession(sessionID.uuidString, appInstanceID: appInstanceID) {
                continuation.resume()
            }
        }
    }

    private static func sendHelloIfNeeded() {
        let shouldSend = xpcLock.withLock { () -> Bool in
            if helloSent {
                return false
            }
            helloSent = true
            return true
        }
        guard shouldSend else { return }

        let connection = getXPCConnection()
        let proxy =
            connection.remoteObjectProxyWithErrorHandler { error in
                beeLog("BROKER XPC appHello error: \(error.localizedDescription)")
                invalidateXPCConnection()
            } as? BeeBrokerXPC
        proxy?.appHello(appInstanceID) { ok in
            if !ok {
                beeLog("BROKER XPC appHello rejected")
            }
        }
    }

    private static func setMarkedTextXPC(_ text: String, sessionID: UUID) {
        Task.detached {
            await withCheckedContinuation { continuation in
                let connection = getXPCConnection()
                let proxy =
                    connection.remoteObjectProxyWithErrorHandler { error in
                        beeLog("BROKER XPC setMarkedText error: \(error.localizedDescription)")
                        invalidateXPCConnection()
                        continuation.resume()
                    } as? BeeBrokerXPC

                guard let proxy else {
                    continuation.resume()
                    return
                }

                proxy.setMarkedText(sessionID.uuidString, text: text, appInstanceID: appInstanceID)
                { _ in
                    continuation.resume()
                }
            }
        }
    }

    private static func commitTextXPC(_ text: String, submit: Bool, sessionID: UUID) {
        Task.detached {
            await withCheckedContinuation { continuation in
                let connection = getXPCConnection()
                let proxy =
                    connection.remoteObjectProxyWithErrorHandler { error in
                        beeLog("BROKER XPC commitText error: \(error.localizedDescription)")
                        invalidateXPCConnection()
                        continuation.resume()
                    } as? BeeBrokerXPC

                guard let proxy else {
                    continuation.resume()
                    return
                }

                proxy.commitText(
                    sessionID.uuidString, text: text, submit: submit, appInstanceID: appInstanceID
                ) { _ in
                    continuation.resume()
                }
            }
        }
    }

    private static func cancelInputXPC(sessionID: UUID) {
        Task.detached {
            await withCheckedContinuation { continuation in
                let connection = getXPCConnection()
                let proxy =
                    connection.remoteObjectProxyWithErrorHandler { error in
                        beeLog("BROKER XPC cancelInput error: \(error.localizedDescription)")
                        invalidateXPCConnection()
                        continuation.resume()
                    } as? BeeBrokerXPC

                guard let proxy else {
                    continuation.resume()
                    return
                }

                proxy.cancelInput(sessionID.uuidString, appInstanceID: appInstanceID) { _ in
                    continuation.resume()
                }
            }
        }
    }

    private static func stopDictatingXPC(sessionID: UUID) {
        Task.detached {
            await withCheckedContinuation { continuation in
                let connection = getXPCConnection()
                let proxy =
                    connection.remoteObjectProxyWithErrorHandler { error in
                        beeLog("BROKER XPC stopDictating error: \(error.localizedDescription)")
                        invalidateXPCConnection()
                        continuation.resume()
                    } as? BeeBrokerXPC

                guard let proxy else {
                    continuation.resume()
                    return
                }

                proxy.stopDictating(sessionID.uuidString, appInstanceID: appInstanceID) { _ in
                    continuation.resume()
                }
            }
        }
    }

    private static func inputSourceID(_ source: TISInputSource) -> String {
        guard let raw = TISGetInputSourceProperty(source, kTISPropertyInputSourceID) else {
            return "<unknown>"
        }
        return Unmanaged<CFString>.fromOpaque(raw).takeUnretainedValue() as String
    }

    private static func findBeeInputSource() -> TISInputSource? {
        let properties: [CFString: Any] = [
            kTISPropertyBundleID: beeBundleID as CFString
        ]
        guard
            let sources = TISCreateInputSourceList(properties as CFDictionary, false)?
                .takeRetainedValue() as? [TISInputSource],
            let source = sources.first
        else {
            return nil
        }
        return source
    }
}
