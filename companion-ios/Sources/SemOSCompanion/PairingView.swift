import SwiftUI

/// Main pairing screen. Handles idle → listening → SAS → paired/failed flow.
struct PairingView: View {
    @StateObject private var listener = PairingListener()
    @State private var phase: PairingPhase = .idle
    @State private var errorMessage: String?

    var body: some View {
        NavigationStack {
            content
                .navigationTitle("SemOS Companion")
        }
    }

    @ViewBuilder
    private var content: some View {
        switch phase {
        case .idle:
            idleView
        case .listening(let ip, let port, let qrString):
            listeningView(ip: ip, port: port, qrString: qrString)
        case .awaitingConfirmation(let sas):
            SASView(
                sas: sas,
                onMatch: { listener.confirmSAS() },
                onMismatch: { listener.rejectSAS() }
            )
        case .paired(let pairingId):
            pairedView(pairingId: pairingId)
        case .failed(let error):
            failedView(error: error)
        }
    }

    private var idleView: some View {
        VStack(spacing: 20) {
            Image(systemName: "iphone.badge.play")
                .font(.system(size: 72))
                .foregroundStyle(.tint)

            Text("Pair this phone with SemOS")
                .font(.title2)
                .multilineTextAlignment(.center)

            Button("Start Pairing") {
                startPairing()
            }
            .buttonStyle(.borderedProminent)
            .controlSize(.large)
        }
        .padding()
    }

    private func listeningView(ip: String, port: UInt16, qrString: String) -> some View {
        ScrollView {
            VStack(spacing: 20) {
                Text("Listening on \(ip):\(port)")
                    .font(.subheadline)
                    .foregroundStyle(.secondary)

                QRCodeView(data: qrString, side: 220)

                Text("Or type this into SemOS:")
                    .font(.caption)
                    .foregroundStyle(.secondary)

                Text(qrString)
                    .font(.system(.body, design: .monospaced))
                    .multilineTextAlignment(.center)
                    .textSelection(.enabled)
                    .padding()
#if canImport(UIKit)
                    .background(Color(.secondarySystemBackground))
#else
                    .background(Color(nsColor: .controlBackgroundColor))
#endif
                    .cornerRadius(8)

                Button("Cancel") {
                    cancelPairing()
                }
                .buttonStyle(.bordered)
                .tint(.red)
            }
            .padding()
        }
    }

    private func pairedView(pairingId: String) -> some View {
        VStack(spacing: 20) {
            Image(systemName: "checkmark.shield.fill")
                .font(.system(size: 72))
                .foregroundStyle(.green)

            Text("Paired with SemOS")
                .font(.title2)

            Text("Pairing ID: \(pairingId)")
                .font(.system(.body, design: .monospaced))
                .textSelection(.enabled)

            Button("Pair Another") {
                phase = .idle
            }
            .buttonStyle(.borderedProminent)
        }
        .padding()
    }

    private func failedView(error: PairingError) -> some View {
        VStack(spacing: 20) {
            Image(systemName: "exclamationmark.triangle.fill")
                .font(.system(size: 72))
                .foregroundStyle(.red)

            Text("Pairing Failed")
                .font(.title2)

            Text(errorMessage ?? "Unknown error")
                .font(.body)
                .multilineTextAlignment(.center)
                .foregroundStyle(.secondary)

            Button("Try Again") {
                phase = .idle
            }
            .buttonStyle(.borderedProminent)
        }
        .padding()
    }

    // MARK: - Actions

    private func startPairing() {
        phase = .idle
        errorMessage = nil

        do {
            let identity = try KeychainIdentity.loadOrCreate()
            listener.start(
                identity: identity,
                onReady: { ip, port, qrString in
                    phase = .listening(ip: ip, port: port, qrString: qrString)
                },
                onEvent: { event in
                    handleEvent(event)
                }
            )
        } catch {
            phase = .failed(.identityLoadFailed)
        }
    }

    private func cancelPairing() {
        listener.stop()
        phase = .idle
    }

    private func handleEvent(_ event: PairingEvent) {
        switch event {
        case .showSAS(let sas):
            phase = .awaitingConfirmation(sas: sas)
        case .paired(let id):
            phase = .paired(pairingId: id)
        case .error(let err):
            errorMessage = description(for: err)
            phase = .failed(err)
        }
    }
}

private func description(for error: PairingError) -> String {
    switch error {
    case .identityLoadFailed: return "Could not load or create identity key."
    case .qrEncodeFailed: return "Could not encode pairing string."
    case .listenerFailed: return "Could not start network listener."
    case .addressResolutionFailed: return "Could not find a local IP address."
    case .handshakeFailed(let msg): return "Handshake failed: \(msg)"
    case .authFailed: return "Authentication failed. Possible MITM."
    case .userRejected: return "SAS did not match."
    }
}
