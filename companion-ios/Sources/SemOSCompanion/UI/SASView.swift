import SwiftUI

/// Display the 6-digit SAS and ask the human to confirm it matches SemOS.
struct SASView: View {
    let sas: String
    let onMatch: () -> Void
    let onMismatch: () -> Void

    var body: some View {
        VStack(spacing: 24) {
            Text("Compare this number with SemOS")
                .font(.headline)

            Text(sas)
                .font(.system(size: 64, weight: .bold, design: .rounded))
                .monospacedDigit()
                .padding()
#if canImport(UIKit)
                .background(Color(.secondarySystemBackground))
#else
                .background(Color(nsColor: .controlBackgroundColor))
#endif
                .cornerRadius(12)

            HStack(spacing: 32) {
                Button(action: onMismatch) {
                    Label("No Match", systemImage: "xmark.circle")
                        .font(.title2)
                }
                .buttonStyle(.bordered)
                .tint(.red)

                Button(action: onMatch) {
                    Label("Match", systemImage: "checkmark.circle")
                        .font(.title2)
                }
                .buttonStyle(.borderedProminent)
                .tint(.green)
            }
        }
        .padding()
    }
}

#if DEBUG
struct SASView_Previews: PreviewProvider {
    static var previews: some View {
        SASView(sas: "239413", onMatch: {}, onMismatch: {})
    }
}
#endif
