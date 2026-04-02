import AppKit
import Foundation

struct TranscriptionHistoryItem: Identifiable, Sendable {
    let id: UUID
    let text: String
    let timestamp: Date
    let appName: String?
    let appIcon: NSImage?

    init(text: String, appName: String? = nil, appIcon: NSImage? = nil) {
        self.id = UUID()
        self.text = text
        self.timestamp = Date()
        self.appName = appName
        self.appIcon = appIcon
    }

    var displayText: String {
        let truncated = text.prefix(60)
        let firstLine = truncated.prefix(while: { $0 != "\n" })
        if firstLine.count < text.count {
            return "..." + firstLine + "..."
        }
        return String(firstLine)
    }
}
