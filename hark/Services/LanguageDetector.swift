import AppKit
import NaturalLanguage
import os

/// Detects the language of visible text in the frontmost window using AX + NaturalLanguage.
@MainActor
struct LanguageDetector {
    private static let logger = Logger(
        subsystem: Bundle.main.bundleIdentifier ?? "hark",
        category: "LanguageDetector"
    )

    /// Map from NLLanguage to Qwen3 language names used by the ASR.
    private static let nlToQwen3: [NLLanguage: String] = [
        .english: "english",
        .french: "french",
        .polish: "polish",
    ]

    /// Maximum number of AX elements to visit when collecting text.
    private static let maxElements = 2000

    /// Result of language detection including the collected text for debugging.
    struct Result {
        let language: String?
        let collectedText: String
        let elementCount: Int
    }

    /// Detect the language from the frontmost window's visible text.
    /// Walks the AX element tree collecting text from all elements,
    /// then runs NLLanguageRecognizer on the result.
    static func detectFromFocusedWindow() -> Result {
        let empty = Result(language: nil, collectedText: "", elementCount: 0)
        guard let app = NSWorkspace.shared.frontmostApplication else { return empty }
        let axApp = AXUIElementCreateApplication(app.processIdentifier)

        // Get the focused window
        var windowRef: AnyObject?
        guard AXUIElementCopyAttributeValue(
            axApp,
            kAXFocusedWindowAttribute as CFString,
            &windowRef
        ) == .success,
              let windowRef,
              CFGetTypeID(windowRef) == AXUIElementGetTypeID()
        else {
            logger.warning("No focused window for \(app.localizedName ?? "?", privacy: .public)")
            return empty
        }
        let window = unsafeBitCast(windowRef, to: AXUIElement.self)

        // Collect text from the element tree
        var texts: [String] = []
        var visited = 0
        collectText(from: window, into: &texts, visited: &visited)

        let combined = texts.joined(separator: " ")
        // Use only the last 500 chars — earlier text is mostly UI chrome.
        let tail = String(combined.suffix(500))
        logger.warning("Collected \(combined.count) chars from \(visited) elements, using last \(tail.count): \(tail.prefix(200), privacy: .public)")

        guard tail.count >= 20 else {
            logger.warning("Not enough text for detection")
            return Result(language: nil, collectedText: combined, elementCount: visited)
        }

        let recognizer = NLLanguageRecognizer()
        recognizer.processString(tail)

        // Log top hypotheses
        let hypotheses = recognizer.languageHypotheses(withMaximum: 3)
        let hypoStr = hypotheses.sorted(by: { $0.value > $1.value }).map { "\($0.key.rawValue)=\(String(format: "%.2f", $0.value))" }.joined(separator: " ")
        logger.warning("Hypotheses: \(hypoStr, privacy: .public)")

        guard let dominant = recognizer.dominantLanguage else {
            logger.warning("No dominant language from \(combined.count) chars")
            return Result(language: nil, collectedText: combined, elementCount: visited)
        }

        let confidence = recognizer.languageHypotheses(withMaximum: 1)[dominant] ?? 0

        guard let qwen3Name = nlToQwen3[dominant] else {
            logger.warning("Detected \(dominant.rawValue, privacy: .public) (conf=\(confidence, format: .fixed(precision: 2))) — not supported")
            return Result(language: nil, collectedText: combined, elementCount: visited)
        }

        // For non-English, require high confidence
        let threshold: Double = (dominant == .english) ? 0.5 : 0.8
        guard confidence >= threshold else {
            logger.warning("\(qwen3Name, privacy: .public) conf=\(confidence, format: .fixed(precision: 2)) below threshold \(threshold, format: .fixed(precision: 2))")
            return Result(language: nil, collectedText: combined, elementCount: visited)
        }

        logger.warning("Detected \(qwen3Name, privacy: .public) (conf=\(confidence, format: .fixed(precision: 2))) from \(combined.count) chars / \(visited) elements")
        return Result(language: qwen3Name, collectedText: combined, elementCount: visited)
    }

    /// Recursively collect text values from an AX element tree.
    private static func collectText(
        from element: AXUIElement,
        into texts: inout [String],
        visited: inout Int
    ) {
        guard visited < maxElements else { return }
        visited += 1

        // Try to read this element's text value
        var valueRef: AnyObject?
        if AXUIElementCopyAttributeValue(element, kAXValueAttribute as CFString, &valueRef) == .success,
           let text = valueRef as? String, !text.isEmpty {
            // Skip placeholder text
            var placeholderRef: AnyObject?
            let isPlaceholder = AXUIElementCopyAttributeValue(element, kAXPlaceholderValueAttribute as CFString, &placeholderRef) == .success
                && (placeholderRef as? String) == text
            if !isPlaceholder {
                texts.append(text)
            }
        }

        // Also try title
        var titleRef: AnyObject?
        if AXUIElementCopyAttributeValue(element, kAXTitleAttribute as CFString, &titleRef) == .success,
           let title = titleRef as? String, !title.isEmpty {
            texts.append(title)
        }

        // Also try description
        var descRef: AnyObject?
        if AXUIElementCopyAttributeValue(element, kAXDescriptionAttribute as CFString, &descRef) == .success,
           let desc = descRef as? String, !desc.isEmpty {
            texts.append(desc)
        }

        // Recurse into children
        var childrenRef: AnyObject?
        guard AXUIElementCopyAttributeValue(element, kAXChildrenAttribute as CFString, &childrenRef) == .success,
              let children = childrenRef as? [AXUIElement] else {
            return
        }

        for child in children {
            guard visited < maxElements else { break }
            collectText(from: child, into: &texts, visited: &visited)
        }
    }
}
