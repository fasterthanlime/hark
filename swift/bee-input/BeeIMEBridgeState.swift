import AppKit
import Foundation

final class BeeIMEBridgeState: NSObject {
    static let shared = BeeIMEBridgeState()

    weak var activeController: BeeInputController?
    private(set) var activeControllerPID: pid_t?
    private(set) var activeClientIdentity: String?
    private(set) var activeSessionID: UUID?
    var pendingText: String?

    var isDictating: Bool {
        activeSessionID != nil
    }

    func attachSession(sessionID: UUID) {
        if activeSessionID != sessionID {
            pendingText = nil
        }
        activeSessionID = sessionID
        beeInputLog("attachSession: session=\(sessionID.uuidString.prefix(8))")
    }

    func clearSessionIfMatching(sessionID: UUID) {
        guard activeSessionID == sessionID else { return }
        beeInputLog("clearSession: session=\(sessionID.uuidString.prefix(8))")
        clearSessionState()
    }

    func flushPending() {
        guard let text = pendingText, let ctrl = activeController else { return }
        beeInputLog("flushPending: delivering \(text.prefix(40).debugDescription)")
        pendingText = nil
        ctrl.handleSetMarkedText(text)
    }

    func setMarkedText(_ text: String, sessionID: UUID) {
        DispatchQueue.main.async {
            guard self.activeSessionID == sessionID else {
                beeInputLog(
                    "setMarkedText: stale session=\(sessionID.uuidString.prefix(8)) current=\(self.activeSessionID?.uuidString.prefix(8) ?? "nil"), dropping"
                )
                return
            }

            if let ctrl = self.activeController {
                ctrl.handleSetMarkedText(text)
            } else {
                beeInputLog("setMarkedText: no controller, queuing \(text.prefix(40).debugDescription)")
                self.pendingText = text
            }
        }
    }

    func commitText(_ text: String, submit: Bool, sessionID: UUID) {
        DispatchQueue.main.async {
            guard self.activeSessionID == sessionID else {
                beeInputLog(
                    "commitText: stale session=\(sessionID.uuidString.prefix(8)) current=\(self.activeSessionID?.uuidString.prefix(8) ?? "nil"), dropping"
                )
                return
            }

            let ctrl = self.activeController
            self.clearSessionState()
            ctrl?.handleCommitText(text, submit: submit)
        }
    }

    func cancelInput(sessionID: UUID) {
        DispatchQueue.main.async {
            guard self.activeSessionID == sessionID else {
                beeInputLog(
                    "cancelInput: stale session=\(sessionID.uuidString.prefix(8)) current=\(self.activeSessionID?.uuidString.prefix(8) ?? "nil"), dropping"
                )
                return
            }

            let ctrl = self.activeController
            self.clearSessionState()
            ctrl?.handleCancelInput()
        }
    }

    func stopDictating(sessionID: UUID) {
        DispatchQueue.main.async {
            guard self.activeSessionID == sessionID else {
                beeInputLog(
                    "stopDictating: stale session=\(sessionID.uuidString.prefix(8)) current=\(self.activeSessionID?.uuidString.prefix(8) ?? "nil"), dropping"
                )
                return
            }
            self.clearSessionState()
        }
    }

    private func clearSessionState() {
        activeSessionID = nil
        pendingText = nil
    }

    func registerActiveController(_ controller: BeeInputController, clientPID: pid_t?, clientIdentity: String?) {
        activeController = controller
        activeControllerPID = clientPID
        activeClientIdentity = clientIdentity
        beeInputLog(
            "registerActiveController: pid=\(clientPID.map(String.init) ?? "nil") clientID=\(clientIdentity ?? "nil")"
        )
    }

    func unregisterActiveController(_ controller: BeeInputController) {
        guard activeController === controller else { return }
        activeController = nil
        activeControllerPID = nil
        activeClientIdentity = nil
    }
}
