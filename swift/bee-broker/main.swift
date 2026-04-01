import Foundation

private func brokerLog(_ msg: String) {
    let ts = ProcessInfo.processInfo.systemUptime
    let line = String(format: "[%.3f] BROKER: %@\n", ts, msg)
    if let data = line.data(using: .utf8),
       let fh = FileHandle(forWritingAtPath: "/tmp/bee.log") {
        fh.seekToEndOfFile()
        fh.write(data)
        fh.closeFile()
    } else if let data = line.data(using: .utf8) {
        try? data.write(to: URL(fileURLWithPath: "/tmp/bee.log"))
    }
}

private final class BeeBrokerService: NSObject, BeeBrokerXPC {
    struct SessionState {
        var appInstanceID: String
        var targetPID: Int32?
        var activationID: String
        var ready: Bool
        var clientPID: Int32?
        var clientID: String?
    }

    private let queue = DispatchQueue(label: "fasterthanlime.bee.broker.state")
    private var appConnections: [String: NSXPCConnection] = [:]
    private var imeConnections: [String: NSXPCConnection] = [:]
    private var sessions: [String: SessionState] = [:]
    private var activeIMEInstanceID: String?

    private func appProxy(_ appInstanceID: String) -> BeeBrokerPeerXPC? {
        guard let conn = appConnections[appInstanceID] else { return nil }
        return conn.remoteObjectProxyWithErrorHandler { error in
            brokerLog("app callback error: \(error.localizedDescription)")
        } as? BeeBrokerPeerXPC
    }

    private func imeProxy() -> BeeBrokerPeerXPC? {
        guard let imeID = activeIMEInstanceID, let conn = imeConnections[imeID] else { return nil }
        return conn.remoteObjectProxyWithErrorHandler { error in
            brokerLog("ime callback error: \(error.localizedDescription)")
        } as? BeeBrokerPeerXPC
    }

    func appHello(_ appInstanceID: String, withReply reply: @escaping (Bool) -> Void) {
        guard let conn = NSXPCConnection.current() else {
            reply(false)
            return
        }
        queue.async {
            self.appConnections[appInstanceID] = conn
            brokerLog("appHello: id=\(appInstanceID.prefix(8))")
            reply(true)
        }
    }

    func imeHello(_ imeInstanceID: String, withReply reply: @escaping (Bool) -> Void) {
        guard let conn = NSXPCConnection.current() else {
            reply(false)
            return
        }
        queue.async {
            self.imeConnections[imeInstanceID] = conn
            self.activeIMEInstanceID = imeInstanceID
            brokerLog("imeHello: id=\(imeInstanceID.prefix(8))")

            // Replay any pending sessions so IME can catch up after connecting late.
            if let ime = self.imeProxy() {
                for (sessionID, state) in self.sessions where !state.ready {
                    ime.handlePrepareSession(
                        sessionID,
                        targetPID: state.targetPID ?? -1,
                        activationID: state.activationID
                    )
                    brokerLog(
                        "imeHello: replay prepare session=\(sessionID.prefix(8)) targetPID=\(state.targetPID.map(String.init) ?? "nil")"
                    )
                }
            }
            reply(true)
        }
    }

    func prepareSession(
        _ sessionID: String,
        targetPID: Int32,
        activationID: String,
        appInstanceID: String,
        withReply reply: @escaping (Bool) -> Void
    ) {
        guard let conn = NSXPCConnection.current() else {
            reply(false)
            return
        }
        queue.async {
            // Bind this app instance to the active caller connection even if appHello
            // has not been observed yet (ordering can race on first use).
            self.appConnections[appInstanceID] = conn
            self.sessions[sessionID] = SessionState(
                appInstanceID: appInstanceID,
                targetPID: targetPID >= 0 ? targetPID : nil,
                activationID: activationID,
                ready: false,
                clientPID: nil,
                clientID: nil
            )

            if let ime = self.imeProxy() {
                brokerLog(
                    "prepareSession: id=\(sessionID.prefix(8)) targetPID=\(targetPID >= 0 ? String(targetPID) : "nil") dispatch=immediate"
                )
                ime.handlePrepareSession(sessionID, targetPID: targetPID, activationID: activationID)
            } else {
                brokerLog(
                    "prepareSession: id=\(sessionID.prefix(8)) targetPID=\(targetPID >= 0 ? String(targetPID) : "nil") dispatch=queued (no IME yet)"
                )
            }
            reply(true)
        }
    }

    func sessionStatus(_ sessionID: String, withReply reply: @escaping (Bool, Int32, String) -> Void) {
        queue.async {
            guard let s = self.sessions[sessionID] else {
                reply(false, -1, "")
                return
            }
            reply(s.ready, s.clientPID ?? -1, s.clientID ?? "")
        }
    }

    func clearSession(_ sessionID: String, appInstanceID: String, withReply reply: @escaping () -> Void) {
        queue.async {
            if let ime = self.imeProxy() {
                ime.handleClearSession(sessionID)
            }
            self.sessions.removeValue(forKey: sessionID)
            reply()
        }
    }

    func setMarkedText(
        _ sessionID: String,
        text: String,
        appInstanceID: String,
        withReply reply: @escaping (Bool) -> Void
    ) {
        queue.async {
            guard let s = self.sessions[sessionID], s.appInstanceID == appInstanceID, let ime = self.imeProxy() else {
                reply(false)
                return
            }
            ime.handleSetMarkedText(sessionID, text: text)
            reply(true)
        }
    }

    func commitText(
        _ sessionID: String,
        text: String,
        submit: Bool,
        appInstanceID: String,
        withReply reply: @escaping (Bool) -> Void
    ) {
        queue.async {
            guard let s = self.sessions[sessionID], s.appInstanceID == appInstanceID, let ime = self.imeProxy() else {
                reply(false)
                return
            }
            ime.handleCommitText(sessionID, text: text, submit: submit)
            self.sessions.removeValue(forKey: sessionID)
            reply(true)
        }
    }

    func cancelInput(_ sessionID: String, appInstanceID: String, withReply reply: @escaping (Bool) -> Void) {
        queue.async {
            guard let s = self.sessions[sessionID], s.appInstanceID == appInstanceID, let ime = self.imeProxy() else {
                reply(false)
                return
            }
            ime.handleCancelInput(sessionID)
            self.sessions.removeValue(forKey: sessionID)
            reply(true)
        }
    }

    func stopDictating(_ sessionID: String, appInstanceID: String, withReply reply: @escaping (Bool) -> Void) {
        queue.async {
            guard let s = self.sessions[sessionID], s.appInstanceID == appInstanceID, let ime = self.imeProxy() else {
                reply(false)
                return
            }
            ime.handleStopDictating(sessionID)
            self.sessions.removeValue(forKey: sessionID)
            reply(true)
        }
    }

    func imeAttach(
        _ sessionID: String,
        clientPID: Int32,
        clientID: String,
        imeInstanceID: String,
        withReply reply: @escaping (Bool) -> Void
    ) {
        guard let conn = NSXPCConnection.current() else {
            reply(false)
            return
        }
        queue.async {
            // Same race-proofing as app side: ensure IME instance is bound by the time
            // it first attaches to a session.
            self.imeConnections[imeInstanceID] = conn
            self.activeIMEInstanceID = imeInstanceID
            guard var s = self.sessions[sessionID], let app = self.appProxy(s.appInstanceID) else {
                reply(false)
                return
            }
            if let target = s.targetPID, target >= 0, clientPID >= 0, target != clientPID {
                brokerLog("imeAttach: pid mismatch session=\(sessionID.prefix(8)) target=\(target) got=\(clientPID)")
                reply(false)
                return
            }
            s.ready = true
            s.clientPID = clientPID >= 0 ? clientPID : nil
            s.clientID = clientID.isEmpty ? nil : clientID
            self.sessions[sessionID] = s
            app.handleIMESessionStarted(sessionID, clientPID: clientPID, clientID: clientID)
            reply(true)
        }
    }

    func imeSubmit(_ sessionID: String, imeInstanceID: String, withReply reply: @escaping () -> Void) {
        queue.async {
            if let s = self.sessions[sessionID], let app = self.appProxy(s.appInstanceID) {
                app.handleIMESubmit(sessionID)
            }
            reply()
        }
    }

    func imeCancel(_ sessionID: String, imeInstanceID: String, withReply reply: @escaping () -> Void) {
        queue.async {
            if let s = self.sessions[sessionID], let app = self.appProxy(s.appInstanceID) {
                app.handleIMECancel(sessionID)
            }
            reply()
        }
    }

    func imeUserTyped(
        _ sessionID: String,
        keyCode: Int32,
        characters: String,
        imeInstanceID: String,
        withReply reply: @escaping () -> Void
    ) {
        queue.async {
            if let s = self.sessions[sessionID], let app = self.appProxy(s.appInstanceID) {
                app.handleIMEUserTyped(sessionID, keyCode: keyCode, characters: characters)
            }
            reply()
        }
    }

    func imeContextLost(
        _ sessionID: String,
        hadMarkedText: Bool,
        imeInstanceID: String,
        withReply reply: @escaping () -> Void
    ) {
        queue.async {
            if let s = self.sessions[sessionID], let app = self.appProxy(s.appInstanceID) {
                app.handleIMEContextLost(sessionID, hadMarkedText: hadMarkedText)
            }
            reply()
        }
    }
}

private final class BeeBrokerDelegate: NSObject, NSXPCListenerDelegate {
    private let service = BeeBrokerService()

    func listener(_ listener: NSXPCListener, shouldAcceptNewConnection newConnection: NSXPCConnection) -> Bool {
        newConnection.exportedInterface = NSXPCInterface(with: BeeBrokerXPC.self)
        newConnection.exportedObject = service
        newConnection.remoteObjectInterface = NSXPCInterface(with: BeeBrokerPeerXPC.self)
        newConnection.resume()
        brokerLog("accepted new connection")
        return true
    }
}

let machService = "fasterthanlime.bee.broker"
let listener = NSXPCListener(machServiceName: machService)
private let delegate = BeeBrokerDelegate()
listener.delegate = delegate
listener.resume()
brokerLog("broker listening machService=\(machService)")
RunLoop.main.run()
