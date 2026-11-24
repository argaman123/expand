// Add PC under the list. Pressed, it turns into pairing mode in place: the 6-digit code in large
// digits, the time left and Cancel, in the same 52pt as the Add PC row so the panel doesn't
// jump. The code shows only while the app reports pairing open and the local countdown
// (re-synced on every update, at most 2:00) hasn't run out.

import SwiftUI

struct PairingView: View {
    let model: PanelModel

    var body: some View {
        TimelineView(.periodic(from: .now, by: 1)) { context in
            if let code = model.pairingCode(at: context.date) {
                HStack(spacing: 12) {
                    HStack(alignment: .firstTextBaseline, spacing: 10) {
                        Text(Self.grouped(code))
                            .font(.system(size: 30, weight: .semibold, design: .rounded))
                            .monospacedDigit()
                            .tracking(2)
                        Text(Self.clock(model.secondsLeft(at: context.date)))
                            .font(.system(size: 12))
                            .foregroundStyle(.secondary)
                            .monospacedDigit()
                    }
                    Spacer(minLength: 8)
                    Button("Cancel") { model.send(.cancelPairing) }
                        .buttonStyle(PillButtonStyle())
                }
                .frame(height: PairingView.rowHeight)
                .padding(.horizontal, 12)
            } else {
                AddPcButton { model.send(.addPc) }
            }
        }
    }

    /// Add PC and pairing take the same height, the same as a PC row.
    static let rowHeight: CGFloat = 52

    /// "123456" as "123 456".
    static func grouped(_ code: String) -> String {
        guard code.count == 6 else { return code }
        return "\(code.prefix(3)) \(code.suffix(3))"
    }

    /// Seconds as "1:52".
    static func clock(_ seconds: Int) -> String {
        String(format: "%d:%02d", seconds / 60, seconds % 60)
    }
}

private struct AddPcButton: View {
    let action: () -> Void
    @State private var hovering = false
    @Environment(\.previewInteraction) private var preview

    private var lit: Bool { hovering || preview != nil }

    var body: some View {
        Button(action: action) {
            HStack(spacing: 12) {
                Image(systemName: "plus")
                    .font(.system(size: 13, weight: .medium))
                    .frame(width: 32, height: 32)
                    .background(lit ? AnyShapeStyle(Brand.accent.opacity(0.28)) : AnyShapeStyle(Color.primary.opacity(0.1)), in: Circle())
                Text("Add device")
                    .font(.system(size: 13))
                Spacer(minLength: 0)
            }
            .frame(height: PairingView.rowHeight)
            .padding(.horizontal, 12)
            .rowHighlight(hovering)
            .contentShape(Rectangle())
        }
        .buttonStyle(PressStyle(shape: RoundedRectangle(cornerRadius: 8, style: .continuous), depth: 0.07, scale: 1))
        .onHover { hovering = $0 }
        .pointerCursor()
    }
}
