import Carbon
import Cocoa
import InputMethodKit

/// Pure pass-through layer. All state lives in BeeIMESession (on the bridge).
/// Per the macOS Input Method Development Guidelines 2026:
/// "IMKInputController must not hold any objects."
@objc(BeeInputController)
class BeeInputController: IMKInputController {

    override func activateServer(_ sender: Any!) {
        beeInputLog("activateServer: entry")
        super.activateServer(sender)
        let bridge = BeeIMEBridgeState.shared
        let frontmostPID = NSWorkspace.shared.frontmostApplication?.processIdentifier
        let clientIdentity = currentClientIdentity()
        // Creates a BeeIMESession and starts the 60ms deferred claim
        bridge.activate(self, pid: frontmostPID, clientID: clientIdentity)
    }

    override func deactivateServer(_ sender: Any!) {
        let bridge = BeeIMEBridgeState.shared

        // Only act on the session if WE are the active controller.
        // A stale controller's deactivateServer must not touch the new session.
        guard bridge.activeController === self else {
            beeInputLog("deactivateServer: stale controller, ignoring")
            super.deactivateServer(sender)
            return
        }

        let session = bridge.currentSession
        let isDictating = bridge.isDictating
        let sessionID = bridge.activeSessionID

        // Cancel any pending claim — the activation was spurious
        session?.cancelPendingClaim()

        let hadMarkedText = !(session?.currentMarkedText.isEmpty ?? true)
        beeInputLog(
            "deactivateServer: session=\(sessionID?.uuidString.prefix(8) ?? "none") hadMarkedText=\(hadMarkedText)"
        )

        // Clear orphaned marked text before deactivating
        session?.clearOrphanedMarkedText()

        bridge.deactivate(self)

        if isDictating, let sessionID {
            if let session {
                session.autoCommittedPrefix = session.currentMarkedText
            }
            BeeBrokerIMEClient.shared.imeContextLost(
                sessionID: sessionID,
                hadMarkedText: hadMarkedText
            )
        }
        super.deactivateServer(sender)
    }

    override func handle(_ event: NSEvent!, client sender: Any!) -> Bool {
        guard let event, event.type == .keyDown else {
            return false
        }
        let bridge = BeeIMEBridgeState.shared
        guard let sessionID = bridge.activeSessionID else {
            return false
        }

        switch Int(event.keyCode) {
        case kVK_Return, kVK_ANSI_KeypadEnter:
            BeeBrokerIMEClient.shared.imeSubmit(sessionID: sessionID)
            return true

        case kVK_Escape:
            BeeBrokerIMEClient.shared.imeCancel(sessionID: sessionID)
            return true

        default:
            BeeBrokerIMEClient.shared.imeUserTyped(
                sessionID: sessionID,
                keyCode: event.keyCode,
                characters: event.characters ?? ""
            )
            return false
        }
    }

    // MARK: - Utilities

    private func currentClientIdentity() -> String? {
        guard let client = self.client() else { return nil }
        let opaque = Unmanaged.passUnretained(client as AnyObject).toOpaque()
        return String(UInt(bitPattern: opaque), radix: 16, uppercase: true)
    }
}
