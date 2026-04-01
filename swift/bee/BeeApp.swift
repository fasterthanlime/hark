import AppKit
import SwiftUI

final class BeeLifecycleDelegate: NSObject, NSApplicationDelegate {
    func applicationWillTerminate(_ notification: Notification) {
        BeeInputClient.switchAwayFromBeeInputIfNeeded()
    }
}

@main
struct BeeApp: App {
    @NSApplicationDelegateAdaptor(BeeLifecycleDelegate.self) private var lifecycleDelegate
    @State private var appState: AppState
    @State private var hotkeyMonitor = HotkeyMonitor()

    init() {
        let audioEngine = AudioEngine()
        let transcriptionService = TranscriptionService()
        let inputClient = BeeInputClient()
        let state = AppState(
            audioEngine: audioEngine,
            transcriptionService: transcriptionService,
            inputClient: inputClient
        )
        _appState = State(initialValue: state)

        let monitor = HotkeyMonitor()
        monitor.appState = state
        monitor.start()
        _hotkeyMonitor = State(initialValue: monitor)

        BeeInputClient.ensureIMERegistered()
        state.loadModelAtStartup()
    }

    var body: some Scene {
        MenuBarExtra {
            MenuBarView(appState: appState)
        } label: {
            menuBarIcon
        }
        .menuBarExtraStyle(.window)
    }

    @ViewBuilder
    private var menuBarIcon: some View {
        let recordingAssetName = "MenuBarIconRecording"
        let defaultAssetName = "MenuBarIcon"
        let preferredAssetName = isActivelyRecording ? recordingAssetName : defaultAssetName
        let resolvedAssetName = NSImage(named: preferredAssetName) == nil ? defaultAssetName : preferredAssetName

        Image(resolvedAssetName)
    }

    private var isActivelyRecording: Bool {
        switch appState.uiState {
        case .pushToTalk, .locked, .lockedOptionHeld:
            return true
        case .idle, .pending:
            return false
        }
    }
}
