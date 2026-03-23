import SwiftUI

/// Result state for overlay dismissal animation.
enum OverlayResult {
    case none
    case success
    case cancelled
}

/// Floating overlay showing transcript with spectrum bars inset at top.
struct RecordingOverlayView: View {
    let appState: AppState

    @State private var isAppearing = false
    @State private var displayedText = ""
    @State private var textAnimationTask: Task<Void, Never>?

    private var dismissResult: OverlayResult { appState.overlayDismiss }

    private var scale: CGFloat {
        if dismissResult == .success { return 1.3 }
        if dismissResult == .cancelled { return 0.7 }
        return isAppearing ? 1.0 : 0.8
    }

    private var opacity: Double {
        if dismissResult != .none { return 0 }
        return isAppearing ? 1.0 : 0.0
    }

    var body: some View {
        mainContent
            .scaleEffect(scale)
            .opacity(opacity)
            .animation(.spring(response: 0.3, dampingFraction: 0.7), value: isAppearing)
            .animation(.easeIn(duration: 0.25), value: dismissResult)
            .frame(width: 700, height: 300)
            .onAppear {
                withAnimation {
                    isAppearing = true
                }
            }
            .onChange(of: appState.partialTranscript) { _, newValue in
                animateTextChange(to: newValue)
            }
    }

    private var mainContent: some View {
        VStack(spacing: 8) {
            // Spectrum bars in a floating circle
            SpectrumBarsView(bands: appState.spectrumBands)

            // Transcript text panel
            Text(displayedTextValue)
                .font(.custom("Jost-Medium", size: 17))
                .foregroundColor(.white)
                .multilineTextAlignment(.leading)
                .fixedSize(horizontal: false, vertical: true)
                .frame(maxWidth: .infinity, alignment: .topLeading)
                .padding(.horizontal, 20)
                .padding(.vertical, 16)
                .frame(width: 500, alignment: .topLeading)
                .background(
                    RoundedRectangle(cornerRadius: 16, style: .continuous)
                        .fill(Color.black.opacity(0.8))
                        .shadow(color: .black.opacity(0.3), radius: 8, y: 4)
                )
        }
    }

    private var displayedTextValue: String {
        if !displayedText.isEmpty { return displayedText }
        if !appState.partialTranscript.isEmpty { return appState.partialTranscript }
        return "Listening..."
    }

    private func animateTextChange(to newText: String) {
        guard !newText.isEmpty else { return }

        textAnimationTask?.cancel()

        let currentText = displayedText
        let commonPrefixLength = currentText.commonPrefix(with: newText).count

        if commonPrefixLength == newText.count {
            displayedText = newText
            return
        }

        let newPart = String(newText.dropFirst(commonPrefixLength))
        displayedText = String(newText.prefix(commonPrefixLength))

        textAnimationTask = Task { @MainActor in
            for char in newPart {
                guard !Task.isCancelled else { return }
                displayedText.append(char)
                try? await Task.sleep(for: .milliseconds(5))
            }
        }
    }
}

/// Six vertical capsule bars inside a circle, tapered at the edges.
struct SpectrumBarsView: View {
    let bands: [Float]

    private let barCount = 6
    private let barWidth: CGFloat = 3
    private let spacing: CGFloat = 2.5
    private let circleSize: CGFloat = 44
    // Taper: outer bars scale down to fit the circle silhouette
    private let taperFactors: [CGFloat] = [0.45, 0.75, 1.0, 1.0, 0.75, 0.45]

    var body: some View {
        ZStack {
            HStack(alignment: .center, spacing: spacing) {
                ForEach(0..<barCount, id: \.self) { index in
                    let level = index < bands.count ? CGFloat(bands[index]) : 0
                    let taper = taperFactors[index]
                    let maxH = (circleSize - 12) * taper
                    let minH: CGFloat = 3
                    let height = minH + (maxH - minH) * level
                    let opacity = 0.5 + 0.5 * level

                    Capsule()
                        .fill(Color.white.opacity(opacity))
                        .frame(width: barWidth, height: height)
                }
            }
            .animation(.easeOut(duration: 0.07), value: bands)
        }
        .frame(width: circleSize, height: circleSize)
        .background(Circle().fill(Color.black.opacity(0.8)))
    }
}
