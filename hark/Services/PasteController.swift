import AppKit
import Carbon.HIToolbox

/// Pastes text into the frontmost application by writing to the pasteboard
/// and simulating Cmd+V.
@MainActor
struct PasteController {

    /// Paste text into the active application.
    /// Saves and restores the original clipboard contents.
    /// - Parameters:
    ///   - text: The text to paste
    ///   - submit: If true, simulate Enter after pasting (for submitting). If false, append a trailing space.
    static func paste(_ text: String, submit: Bool = false) async throws {
        guard hasAccessibilityPermission else {
            throw PasteError.accessibilityPermissionRequired
        }

        guard !text.isEmpty else { return }

        let pasteboard = NSPasteboard.general
        let snapshot = capturePasteboardSnapshot(from: pasteboard)

        // Append a trailing space unless we're submitting
        var finalText = text
        if !submit {
            finalText += " "
        }

        guard writeText(finalText, to: pasteboard) else {
            throw PasteError.pasteboardWriteFailed
        }

        let stagedChangeCount = pasteboard.changeCount

        // Small delay to ensure pasteboard is updated
        try? await Task.sleep(for: .milliseconds(50))

        // Simulate Cmd+V keystroke
        do {
            try await simulateCmdV()
        } catch {
            restorePasteboard(
                snapshot,
                to: pasteboard,
                expectedChangeCount: stagedChangeCount
            )
            throw error
        }

        // If submitting, simulate Enter after paste
        if submit {
            try? await Task.sleep(for: .milliseconds(300))
            try await simulateReturn()
        }

        // Restore original pasteboard after a delay (don't block the caller)
        let changeCount = stagedChangeCount
        Task { @MainActor in
            try? await Task.sleep(for: .milliseconds(300))
            restorePasteboard(snapshot, to: pasteboard, expectedChangeCount: changeCount)
        }
    }

    /// Check if the Return/Enter key is currently pressed.
    static func isReturnKeyPressed() -> Bool {
        guard let source = CGEventSource(stateID: .hidSystemState) else {
            return false
        }
        return CGEventSource.keyState(.hidSystemState, key: CGKeyCode(kVK_Return))
    }

    /// Check if the app has Accessibility permission (required for CGEvent posting).
    static var hasAccessibilityPermission: Bool {
        AXIsProcessTrusted()
    }

    /// Prompt the user to grant Accessibility permission.
    static func requestAccessibilityPermission() {
        // Use string literal to avoid Swift 6 concurrency warning on global kAXTrustedCheckOptionPrompt
        let options = ["AXTrustedCheckOptionPrompt": true] as CFDictionary
        AXIsProcessTrustedWithOptions(options)
    }

    // MARK: - Direct AX input

    /// Result of capturing the focused AX text element.
    struct CapturedTextField {
        let element: AXUIElement
        /// The real text content (empty if only placeholder was present).
        let text: String
        /// The cursor position (UTF-16 offset).
        let cursorPosition: Int
    }

    /// Captures the focused AX text element so we can write to it repeatedly
    /// during streaming transcription. Returns nil if the focused element
    /// doesn't support text value manipulation.
    static func captureFocusedTextField() -> CapturedTextField? {
        let systemWide = AXUIElementCreateSystemWide()
        var focusedRef: AnyObject?
        guard AXUIElementCopyAttributeValue(
            systemWide,
            kAXFocusedUIElementAttribute as CFString,
            &focusedRef
        ) == .success,
              let focusedRef,
              CFGetTypeID(focusedRef) == AXUIElementGetTypeID()
        else {
            return nil
        }
        let element = unsafeBitCast(focusedRef, to: AXUIElement.self)

        // Verify it supports kAXValueAttribute (i.e. it's a text field)
        var valueRef: AnyObject?
        guard AXUIElementCopyAttributeValue(
            element,
            kAXValueAttribute as CFString,
            &valueRef
        ) == .success, let rawValue = valueRef as? String else {
            return nil
        }

        // Check if the value is just placeholder text — if so, treat as empty.
        var placeholderRef: AnyObject?
        let text: String
        if AXUIElementCopyAttributeValue(
            element,
            kAXPlaceholderValueAttribute as CFString,
            &placeholderRef
        ) == .success, let placeholder = placeholderRef as? String, rawValue == placeholder {
            text = ""
        } else {
            text = rawValue
        }

        // Read cursor position
        var cursorPosition = (text as NSString).length
        var rangeRef: AnyObject?
        if AXUIElementCopyAttributeValue(element, kAXSelectedTextRangeAttribute as CFString, &rangeRef) == .success,
           let rangeRef, CFGetTypeID(rangeRef) == AXValueGetTypeID() {
            let rangeValue = unsafeBitCast(rangeRef, to: AXValue.self)
            var range = CFRange(location: 0, length: 0)
            if AXValueGetValue(rangeValue, .cfRange, &range) {
                cursorPosition = min(range.location, (text as NSString).length)
            }
        }

        return CapturedTextField(element: element, text: text, cursorPosition: cursorPosition)
    }

    /// Write text directly into a text field using keyboard simulation.
    /// Computes the diff between `previousText` and `text`:
    /// - Common prefix is left alone
    /// - Changed tail is deleted with Delete keypresses, then new tail is typed
    /// - No AX value rewriting, no selection flashing
    @discardableResult
    static func setDirectText(
        _ text: String,
        previousText: String,
        on element: AXUIElement,
        replaceFrom: Int
    ) -> Bool {
        if previousText.isEmpty && text.isEmpty { return true }

        // Find where old and new text diverge
        let commonPrefix = previousText.commonPrefix(with: text)
        let oldTail = String(previousText[commonPrefix.endIndex...])
        let newTail = String(text[commonPrefix.endIndex...])

        if previousText.isEmpty {
            // First insertion: position cursor via AX, then type
            var range = CFRange(location: replaceFrom, length: 0)
            if let rangeValue = AXValueCreate(.cfRange, &range) {
                AXUIElementSetAttributeValue(
                    element,
                    kAXSelectedTextRangeAttribute as CFString,
                    rangeValue
                )
            }
            typeText(newTail)
            return true
        }

        // Delete the old tail using Option+Delete (word-level) + Delete (remainder)
        if !oldTail.isEmpty {
            deleteBackward(text: oldTail)
        }

        // Type the new tail
        if !newTail.isEmpty {
            typeText(newTail)
        }

        return true
    }

    /// Delete `text` backward from the cursor. Uses Option+Delete for word
    /// chunks and plain Delete for individual characters. Simulates macOS
    /// word boundary behavior: Option+Delete deletes back through whitespace
    /// and then through the preceding word/punctuation.
    private static func deleteBackward(text: String) {
        guard !text.isEmpty else { return }
        let deleteKeyCode = CGKeyCode(kVK_Delete)

        // Walk backward through the text, simulating what Option+Delete would
        // consume each time. Each Option+Delete eats: any trailing whitespace,
        // then the preceding word or punctuation run.
        var remaining = text[text.startIndex..<text.endIndex]
        var optionDeleteCount = 0

        while !remaining.isEmpty {
            // Strip trailing whitespace
            while let last = remaining.last, last.isWhitespace {
                remaining = remaining[remaining.startIndex..<remaining.index(before: remaining.endIndex)]
            }
            if remaining.isEmpty {
                // Only whitespace was left — one more Option+Delete
                optionDeleteCount += 1
                break
            }

            // Now strip the preceding "word" — either alphanumerics or a
            // punctuation/symbol run (Option+Delete treats these as separate units)
            let lastChar = remaining.last!
            if lastChar.isLetter || lastChar.isNumber {
                while let last = remaining.last, last.isLetter || last.isNumber {
                    remaining = remaining[remaining.startIndex..<remaining.index(before: remaining.endIndex)]
                }
            } else {
                // Punctuation/symbol — Option+Delete eats the run
                while let last = remaining.last, !last.isLetter && !last.isNumber && !last.isWhitespace {
                    remaining = remaining[remaining.startIndex..<remaining.index(before: remaining.endIndex)]
                }
            }
            optionDeleteCount += 1
        }

        for _ in 0..<optionDeleteCount {
            guard let keyDown = CGEvent(keyboardEventSource: nil, virtualKey: deleteKeyCode, keyDown: true),
                  let keyUp = CGEvent(keyboardEventSource: nil, virtualKey: deleteKeyCode, keyDown: false)
            else { continue }
            keyDown.flags = .maskAlternate
            keyUp.flags = .maskAlternate
            keyDown.post(tap: .cghidEventTap)
            keyUp.post(tap: .cghidEventTap)
        }
    }

    /// Type text via CGEvent unicode keyboard events.
    private static func typeText(_ text: String) {
        let utf16 = Array(text.utf16)
        let chunkSize = 20
        for offset in stride(from: 0, to: utf16.count, by: chunkSize) {
            let end = min(offset + chunkSize, utf16.count)
            var chunk = Array(utf16[offset..<end])

            guard let keyDown = CGEvent(keyboardEventSource: nil, virtualKey: 0, keyDown: true) else { continue }
            keyDown.flags = []
            keyDown.keyboardSetUnicodeString(stringLength: chunk.count, unicodeString: &chunk)
            keyDown.post(tap: .cghidEventTap)

            guard let keyUp = CGEvent(keyboardEventSource: nil, virtualKey: 0, keyDown: false) else { continue }
            keyUp.flags = []
            keyUp.post(tap: .cghidEventTap)
        }
    }

    // MARK: - Private

    /// Check whether the focused text field already has non-whitespace text
    /// immediately before the cursor, meaning we should prepend a space to the
    /// transcribed text so it doesn't jam against existing content.
    private static func shouldPrependSpace() -> Bool {
        let systemWide = AXUIElementCreateSystemWide()

        // Get the currently focused UI element
        var focusedRef: AnyObject?
        guard AXUIElementCopyAttributeValue(
            systemWide,
            kAXFocusedUIElementAttribute as CFString,
            &focusedRef
        ) == .success,
              let focusedRef
        else {
            return false
        }
        guard CFGetTypeID(focusedRef) == AXUIElementGetTypeID() else {
            return false
        }
        let element = unsafeBitCast(focusedRef, to: AXUIElement.self)

        // Read the element's text value
        var valueRef: AnyObject?
        guard AXUIElementCopyAttributeValue(
            element,
            kAXValueAttribute as CFString,
            &valueRef
        ) == .success,
              let text = valueRef as? String,
              !text.isEmpty
        else {
            return false
        }

        // Read the selected text range (cursor position)
        var rangeRef: AnyObject?
        guard AXUIElementCopyAttributeValue(
            element,
            kAXSelectedTextRangeAttribute as CFString,
            &rangeRef
        ) == .success,
              let rangeRef
        else {
            return false
        }
        guard CFGetTypeID(rangeRef) == AXValueGetTypeID() else {
            return false
        }
        let rangeValue = unsafeBitCast(rangeRef, to: AXValue.self)
        guard AXValueGetType(rangeValue) == .cfRange else {
            return false
        }
        var range = CFRange(location: 0, length: 0)
        guard AXValueGetValue(rangeValue, .cfRange, &range) else {
            return false
        }

        let cursorPosition = range.location
        guard cursorPosition > 0, cursorPosition <= text.utf16.count else {
            return false
        }

        // Get the character just before the cursor
        let utf16 = text.utf16
        let idx = utf16.index(utf16.startIndex, offsetBy: cursorPosition - 1)
        guard let scalar = Unicode.Scalar(utf16[idx]) else { return false }
        let char = Character(scalar)

        return !char.isWhitespace
    }

    private static func writeText(_ text: String, to pasteboard: NSPasteboard) -> Bool {
        pasteboard.clearContents()
        return pasteboard.setString(text, forType: .string)
    }

    private static func capturePasteboardSnapshot(from pasteboard: NSPasteboard) -> PasteboardSnapshot {
        let snapshots = (pasteboard.pasteboardItems ?? []).map { item in
            let payloads = item.types.compactMap { type -> PasteboardPayload? in
                guard let data = item.data(forType: type) else { return nil }
                return PasteboardPayload(type: type, data: data)
            }
            return PasteboardItemSnapshot(payloads: payloads)
        }

        return PasteboardSnapshot(items: snapshots)
    }

    private static func restorePasteboard(
        _ snapshot: PasteboardSnapshot,
        to pasteboard: NSPasteboard,
        expectedChangeCount: Int
    ) {
        guard pasteboard.changeCount == expectedChangeCount else {
            return
        }

        pasteboard.clearContents()
        guard !snapshot.items.isEmpty else {
            return
        }

        let restoredItems: [NSPasteboardItem] = snapshot.items.compactMap { snapshotItem in
            guard !snapshotItem.payloads.isEmpty else { return nil }
            let item = NSPasteboardItem()
            for payload in snapshotItem.payloads {
                item.setData(payload.data, forType: payload.type)
            }
            return item
        }

        if !restoredItems.isEmpty {
            pasteboard.writeObjects(restoredItems)
        }
    }

    private static func simulateCmdV() async throws {
        let vKeyCode: CGKeyCode = CGKeyCode(kVK_ANSI_V)

        // Key down
        guard let keyDown = CGEvent(
            keyboardEventSource: nil,
            virtualKey: vKeyCode,
            keyDown: true
        ) else {
            throw PasteError.keyEventCreationFailed
        }
        keyDown.flags = .maskCommand
        keyDown.post(tap: .cghidEventTap)

        // Brief delay between down and up
        try? await Task.sleep(for: .milliseconds(10))

        // Key up
        guard let keyUp = CGEvent(
            keyboardEventSource: nil,
            virtualKey: vKeyCode,
            keyDown: false
        ) else {
            throw PasteError.keyEventCreationFailed
        }
        keyUp.flags = .maskCommand
        keyUp.post(tap: .cghidEventTap)
    }

    private static func simulateReturn() async throws {
        let returnKeyCode: CGKeyCode = CGKeyCode(kVK_Return)

        guard let keyDown = CGEvent(
            keyboardEventSource: nil,
            virtualKey: returnKeyCode,
            keyDown: true
        ) else {
            throw PasteError.keyEventCreationFailed
        }
        keyDown.flags = []  // Clear all modifiers — bare Return only.
        keyDown.post(tap: .cghidEventTap)

        try? await Task.sleep(for: .milliseconds(10))

        guard let keyUp = CGEvent(
            keyboardEventSource: nil,
            virtualKey: returnKeyCode,
            keyDown: false
        ) else {
            throw PasteError.keyEventCreationFailed
        }
        keyUp.flags = []
        keyUp.post(tap: .cghidEventTap)
    }

    private struct PasteboardSnapshot {
        let items: [PasteboardItemSnapshot]
    }

    private struct PasteboardItemSnapshot {
        let payloads: [PasteboardPayload]
    }

    private struct PasteboardPayload {
        let type: NSPasteboard.PasteboardType
        let data: Data
    }
}

enum PasteError: LocalizedError {
    case accessibilityPermissionRequired
    case pasteboardWriteFailed
    case keyEventCreationFailed

    var errorDescription: String? {
        switch self {
        case .accessibilityPermissionRequired:
            return "Accessibility permission is required to paste text"
        case .pasteboardWriteFailed:
            return "Unable to write text to the pasteboard"
        case .keyEventCreationFailed:
            return "Unable to synthesize Cmd+V key events"
        }
    }
}
