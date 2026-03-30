import Cocoa
import InputMethodKit
import os

/// IMK input controller for Hark dictation.
/// Receives text from the main Hark app via XPC and inserts it into
/// the client application using setMarkedText / insertText.
@objc(HarkInputController)
class HarkInputController: IMKInputController {
    private static let logger = Logger(
        subsystem: "fasterthanlime.hark.input-method",
        category: "InputController"
    )

    /// The current marked (provisional) text, if any.
    private var currentMarkedText: String = ""
    /// Text that was auto-committed by the app during a focus switch.
    /// Stripped from the next setMarkedText to avoid duplication.
    private var autoCommittedPrefix: String = ""

    // MARK: - Lifecycle

    override func activateServer(_ sender: Any!) {
        super.activateServer(sender)
        HarkXPCService.shared.activeController = self
        HarkXPCService.shared.lastController = self
        Self.logger.warning("Server activated")
    }

    override func deactivateServer(_ sender: Any!) {
        let hadMarkedText = !currentMarkedText.isEmpty

        // If the app is going to auto-commit our marked text on focus loss,
        // remember it so we can strip the prefix when streaming resumes.
        if hadMarkedText && HarkXPCService.shared.isDictating {
            autoCommittedPrefix = currentMarkedText
            Self.logger.warning("Saving auto-committed prefix (\(self.autoCommittedPrefix.count) chars)")
        }
        currentMarkedText = ""

        if !hadMarkedText {
            if HarkXPCService.shared.activeController === self {
                HarkXPCService.shared.activeController = nil
            }
        }
        super.deactivateServer(sender)
        Self.logger.warning("Server deactivated (hadMarkedText=\(hadMarkedText ? "yes" : "no"))")
    }

    // MARK: - Input handling

    /// We don't handle regular key input — just pass it through to the app.
    override func inputText(_ string: String!, client sender: Any!) -> Bool {
        return false
    }

    /// Handle key events — intercept Enter/Escape during active dictation.
    override func handle(_ event: NSEvent!, client sender: Any!) -> Bool {
        guard let event, event.type == .keyDown, HarkXPCService.shared.isDictating else {
            return false
        }

        switch Int(event.keyCode) {
        case kVK_Return, kVK_ANSI_KeypadEnter:
            // Tell Hark to finalize + submit
            Self.logger.warning("Enter pressed during dictation — requesting submit")
            DistributedNotificationCenter.default().postNotificationName(
                NSNotification.Name("fasterthanlime.hark.imeSubmit"),
                object: nil,
                userInfo: nil,
                deliverImmediately: true
            )
            return true

        case kVK_Escape:
            // Tell Hark to cancel
            Self.logger.warning("Escape pressed during dictation — requesting cancel")
            DistributedNotificationCenter.default().postNotificationName(
                NSNotification.Name("fasterthanlime.hark.imeCancel"),
                object: nil,
                userInfo: nil,
                deliverImmediately: true
            )
            return true

        default:
            return false
        }
    }

    // MARK: - Commands from Hark via XPC

    /// Set provisional text (streaming transcription updates).
    func handleSetMarkedText(_ text: String) {
        guard let client = self.client() else {
            Self.logger.warning("No client for setMarkedText")
            return
        }

        // If the app auto-committed text on focus loss, strip that prefix
        // to avoid duplication.
        var displayText = text
        if !autoCommittedPrefix.isEmpty {
            if text.hasPrefix(autoCommittedPrefix) {
                displayText = String(text.dropFirst(autoCommittedPrefix.count))
                // Trim leading whitespace from the remainder
                displayText = String(displayText.drop(while: { $0 == " " }))
                Self.logger.warning("Stripped auto-committed prefix, showing \(displayText.count) chars")
            } else {
                // Text diverged from what was committed — clear the prefix
                Self.logger.warning("Auto-committed prefix no longer matches, clearing")
            }
            autoCommittedPrefix = ""
        }

        currentMarkedText = displayText

        // Create attributed string with underline to indicate provisional text
        let attrs: [NSAttributedString.Key: Any] = [
            .underlineStyle: NSUnderlineStyle.single.rawValue,
            .underlineColor: NSColor.systemBlue.withAlphaComponent(0.5),
        ]
        let attributed = NSAttributedString(string: displayText, attributes: attrs)

        client.setMarkedText(
            attributed,
            selectionRange: NSRange(location: displayText.utf16.count, length: 0),
            replacementRange: NSRange(location: NSNotFound, length: 0)
        )
    }

    /// Commit final text, replacing marked text.
    func handleCommitText(_ text: String, submit: Bool = false) {
        guard let client = self.client() else {
            Self.logger.warning("No client for commitText")
            return
        }

        // Strip auto-committed prefix from final text too
        var finalText = text
        if !autoCommittedPrefix.isEmpty {
            if text.hasPrefix(autoCommittedPrefix) {
                finalText = String(text.dropFirst(autoCommittedPrefix.count))
                finalText = String(finalText.drop(while: { $0 == " " }))
            }
            autoCommittedPrefix = ""
        }

        currentMarkedText = ""
        client.insertText(
            finalText + " ",
            replacementRange: NSRange(location: NSNotFound, length: 0)
        )
    }

    /// Cancel — clear marked text without committing.
    func handleCancelInput() {
        guard let client = self.client() else {
            Self.logger.warning("No client for cancelInput")
            return
        }

        currentMarkedText = ""
        // Setting empty marked text clears it
        client.setMarkedText(
            "",
            selectionRange: NSRange(location: 0, length: 0),
            replacementRange: NSRange(location: NSNotFound, length: 0)
        )
    }
}
