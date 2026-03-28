import AppKit
import SwiftUI

/// Manages the lifecycle of floating recording indicator panels — one per screen.
@MainActor
final class OverlayManager {
    private var panels: [FloatingPanel<AnyView>] = []
    private var isPresented = false
    private var currentAppState: AppState?
    private let overlaySize = CGSize(width: 700, height: 300)
    private var dismissTask: Task<Void, Never>?

    func show(appState: AppState) {
        // Cancel any pending dismiss from a previous recording.
        dismissTask?.cancel()
        dismissTask = nil

        currentAppState = appState
        appState.overlayDismiss = .none

        // Close stale panels.
        if !panels.isEmpty {
            for panel in panels { panel.close() }
            panels.removeAll()
        }

        let anchorPoint = Self.focusAnchorPoint() ?? NSEvent.mouseLocation
        let targetScreen = NSScreen.screens.first(where: { NSMouseInRect(anchorPoint, $0.frame, false) })
            ?? NSScreen.main

        guard let targetScreen else { return }

        let binding = Binding<Bool>(
            get: { [weak self] in
                self?.isPresented ?? false
            },
            set: { [weak self] newValue in
                guard let self else { return }
                self.isPresented = newValue
                if !newValue {
                    self.panels.removeAll()
                }
            }
        )

        let contentRect = NSRect(origin: .zero, size: overlaySize)
        let panel = FloatingPanel(
            view: {
                AnyView(
                    RecordingOverlayView(appState: appState)
                )
            },
            contentRect: contentRect,
            isPresented: binding
        )
        panel.positionNearCursor(anchorPoint, on: targetScreen)
        panel.alphaValue = 1.0
        panel.orderFrontRegardless()
        panels.append(panel)

        isPresented = true
    }

    func hide() {
        dismissTask?.cancel()
        dismissTask = nil
        isPresented = false
        for panel in panels { panel.close() }
        panels.removeAll()
        currentAppState?.overlayDismiss = .none
        currentAppState = nil
    }

    /// Trigger dismiss animation and return immediately (non-blocking).
    func hideWithResult(_ result: OverlayResult) {
        guard result != .none, let appState = currentAppState else {
            hide()
            return
        }

        // Tell the SwiftUI views to animate the dismiss (all panels share appState).
        appState.overlayDismiss = result

        // Schedule cleanup after the animation plays.
        dismissTask = Task { @MainActor in
            try? await Task.sleep(for: .milliseconds(280))
            guard !Task.isCancelled else { return }
            self.hide()
        }
    }

    private static func focusAnchorPoint() -> NSPoint? {
        guard AXIsProcessTrusted() else { return nil }

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

        if let caretPoint = caretAnchorPoint(for: element) {
            return caretPoint
        }

        return elementFrameAnchorPoint(for: element)
    }

    private static func caretAnchorPoint(for element: AXUIElement) -> NSPoint? {
        var rangeRef: AnyObject?
        guard AXUIElementCopyAttributeValue(
            element,
            kAXSelectedTextRangeAttribute as CFString,
            &rangeRef
        ) == .success,
        let rangeRef,
        CFGetTypeID(rangeRef) == AXValueGetTypeID()
        else {
            return nil
        }

        let rangeValue = unsafeBitCast(rangeRef, to: AXValue.self)
        guard AXValueGetType(rangeValue) == .cfRange else {
            return nil
        }

        var boundsRef: AnyObject?
        let status = AXUIElementCopyParameterizedAttributeValue(
            element,
            kAXBoundsForRangeParameterizedAttribute as CFString,
            rangeValue,
            &boundsRef
        )
        guard status == .success,
              let boundsRef,
              CFGetTypeID(boundsRef) == AXValueGetTypeID()
        else {
            return nil
        }

        let boundsValue = unsafeBitCast(boundsRef, to: AXValue.self)
        guard AXValueGetType(boundsValue) == .cgRect else {
            return nil
        }

        var rect = CGRect.zero
        guard AXValueGetValue(boundsValue, .cgRect, &rect) else {
            return nil
        }

        guard rect.width > 0 || rect.height > 0 else { return nil }
        return NSPoint(x: rect.midX, y: rect.maxY)
    }

    private static func elementFrameAnchorPoint(for element: AXUIElement) -> NSPoint? {
        var frameRef: AnyObject?
        guard AXUIElementCopyAttributeValue(
            element,
            "AXFrame" as CFString,
            &frameRef
        ) == .success,
        let frameRef,
        CFGetTypeID(frameRef) == AXValueGetTypeID()
        else {
            return nil
        }

        let frameValue = unsafeBitCast(frameRef, to: AXValue.self)
        guard AXValueGetType(frameValue) == .cgRect else {
            return nil
        }

        var rect = CGRect.zero
        guard AXValueGetValue(frameValue, .cgRect, &rect) else {
            return nil
        }

        guard rect.width > 0 || rect.height > 0 else { return nil }
        return NSPoint(x: rect.midX, y: rect.maxY)
    }
}
