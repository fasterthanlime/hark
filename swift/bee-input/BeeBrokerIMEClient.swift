import Foundation

final class BeeBrokerIMEClient {
    static let shared = BeeBrokerIMEClient()

    private let brokerServiceName = "fasterthanlime.bee.broker"
    private let imeInstanceID = UUID().uuidString
    private let lock = NSLock()
    private var connection: NSXPCConnection?
    private var started = false
    private let callbackSink = BeeIMEPeerSink()

    private init() {}

    func start() {
        let shouldStart = lock.withLock { () -> Bool in
            if started { return false }
            started = true
            return true
        }
        guard shouldStart else { return }
        sendHello()
    }

    private func getConnection() -> NSXPCConnection {
        lock.withLock {
            if let connection {
                return connection
            }
            let conn = NSXPCConnection(machServiceName: brokerServiceName, options: [])
            conn.remoteObjectInterface = NSXPCInterface(with: BeeBrokerXPC.self)
            conn.exportedInterface = NSXPCInterface(with: BeeBrokerPeerXPC.self)
            conn.exportedObject = callbackSink
            conn.resume()
            connection = conn
            return conn
        }
    }

    private func invalidateConnection() {
        lock.withLock {
            connection?.invalidate()
            connection = nil
        }
    }

    private func sendHello() {
        let conn = getConnection()
        let proxy = conn.remoteObjectProxyWithErrorHandler { error in
            beeInputLog("BROKER imeHello error: \(error.localizedDescription)")
            self.invalidateConnection()
        } as? BeeBrokerXPC
        proxy?.imeHello(imeInstanceID) { ok in
            if !ok {
                beeInputLog("BROKER imeHello rejected")
            } else {
                beeInputLog("BROKER imeHello ok id=\(self.imeInstanceID.prefix(8))")
            }
        }
    }

    func imeAttach(sessionID: UUID, clientPID: pid_t?, clientID: String?) {
        let conn = getConnection()
        let proxy = conn.remoteObjectProxyWithErrorHandler { error in
            beeInputLog("BROKER imeAttach error: \(error.localizedDescription)")
            self.invalidateConnection()
        } as? BeeBrokerXPC
        proxy?.imeAttach(
            sessionID.uuidString,
            clientPID: clientPID.map { Int32($0) } ?? -1,
            clientID: clientID ?? "",
            imeInstanceID: imeInstanceID
        ) { ok in
            if !ok {
                beeInputLog("BROKER imeAttach rejected session=\(sessionID.uuidString.prefix(8))")
            }
        }
    }

    func imeSubmit(sessionID: UUID) {
        let conn = getConnection()
        let proxy = conn.remoteObjectProxyWithErrorHandler { _ in } as? BeeBrokerXPC
        proxy?.imeSubmit(sessionID.uuidString, imeInstanceID: imeInstanceID) {}
    }

    func imeCancel(sessionID: UUID) {
        let conn = getConnection()
        let proxy = conn.remoteObjectProxyWithErrorHandler { _ in } as? BeeBrokerXPC
        proxy?.imeCancel(sessionID.uuidString, imeInstanceID: imeInstanceID) {}
    }

    func imeUserTyped(sessionID: UUID, keyCode: UInt16, characters: String) {
        let conn = getConnection()
        let proxy = conn.remoteObjectProxyWithErrorHandler { _ in } as? BeeBrokerXPC
        proxy?.imeUserTyped(
            sessionID.uuidString,
            keyCode: Int32(keyCode),
            characters: characters,
            imeInstanceID: imeInstanceID
        ) {}
    }

    func imeContextLost(sessionID: UUID, hadMarkedText: Bool) {
        let conn = getConnection()
        let proxy = conn.remoteObjectProxyWithErrorHandler { _ in } as? BeeBrokerXPC
        proxy?.imeContextLost(
            sessionID.uuidString,
            hadMarkedText: hadMarkedText,
            imeInstanceID: imeInstanceID
        ) {}
    }
}

private final class BeeIMEPeerSink: NSObject, BeeBrokerPeerXPC {
    func handlePrepareSession(_ sessionID: String, targetPID: Int32, activationID: String) {
        guard let id = UUID(uuidString: sessionID) else { return }
        let pid: pid_t? = targetPID >= 0 ? pid_t(targetPID) : nil
        BeeIMEBridgeState.shared.prepareSession(sessionID: id, targetPID: pid, activationID: activationID)
    }

    func handleClearSession(_ sessionID: String) {
        guard let id = UUID(uuidString: sessionID) else { return }
        BeeIMEBridgeState.shared.clearSessionIfMatching(sessionID: id)
    }

    func handleSetMarkedText(_ sessionID: String, text: String) {
        guard let id = UUID(uuidString: sessionID) else { return }
        BeeIMEBridgeState.shared.setMarkedText(text, sessionID: id)
    }

    func handleCommitText(_ sessionID: String, text: String, submit: Bool) {
        guard let id = UUID(uuidString: sessionID) else { return }
        BeeIMEBridgeState.shared.commitText(text, submit: submit, sessionID: id)
    }

    func handleCancelInput(_ sessionID: String) {
        guard let id = UUID(uuidString: sessionID) else { return }
        BeeIMEBridgeState.shared.cancelInput(sessionID: id)
    }

    func handleStopDictating(_ sessionID: String) {
        guard let id = UUID(uuidString: sessionID) else { return }
        BeeIMEBridgeState.shared.stopDictating(sessionID: id)
    }

    func handleIMESessionStarted(_ sessionID: String, clientPID: Int32, clientID: String) {}
    func handleIMESubmit(_ sessionID: String) {}
    func handleIMECancel(_ sessionID: String) {}
    func handleIMEUserTyped(_ sessionID: String, keyCode: Int32, characters: String) {}
    func handleIMEContextLost(_ sessionID: String, hadMarkedText: Bool) {}
}
