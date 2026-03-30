import SwiftUI

@main
struct BeeApp: App {
    @State private var appState: AppState

    init() {
        let audioEngine = AudioEngine()
        let transcriptionService = TranscriptionService()
        let inputClient = BeeInputClient()
        _appState = State(initialValue: AppState(
            audioEngine: audioEngine,
            transcriptionService: transcriptionService,
            inputClient: inputClient
        ))
    }

    var body: some Scene {
        MenuBarExtra {
            MenuBarView(appState: appState)
        } label: {
            Image("MenuBarIcon")
        }
    }
}

struct MenuBarView: View {
    let appState: AppState

    var body: some View {
        Text(statusText)
            .font(.headline)

        Divider()

        Section("Recent") {
            Text("No recent transcriptions")
                .foregroundStyle(.secondary)
        }

        Divider()

        Section("Settings") {
            Text("TODO: model, device, toggles")
                .foregroundStyle(.secondary)
        }

        Divider()

        Button("Quit Bee") {
            BeeInputClient.restoreInputSourceIfNeeded()
            NSApplication.shared.terminate(nil)
        }
    }

    private var statusText: String {
        switch appState.uiState {
        case .idle: "Bee Ready"
        case .pending: "Bee Starting..."
        case .pushToTalk: "Bee Recording"
        case .locked: "Bee Recording (Locked)"
        case .lockedOptionHeld: "Bee Recording (Locked)"
        }
    }
}
