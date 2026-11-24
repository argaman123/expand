// This Mac: Stay awake while connected, and Start at Login. A switch moves at once; the app's
// next state corrects it if the change didn't take.

import SwiftUI

struct ThisMacSection: View {
    let state: PanelDto
    let model: PanelModel
    @Environment(\.offscreen) private var offscreen

    var body: some View {
        PanelSection(title: "This Mac") {
            BoxLine(label: "Stay awake while connected", action: { model.send(.setStayAwake(on: !state.stayAwake)) }) {
                toggle("Stay awake while connected", state.stayAwake) { model.send(.setStayAwake(on: $0)) }
            }
            BoxDivider()
            BoxLine(label: "Start at Login", action: { model.send(.setStartAtLogin(on: !state.startAtLogin)) }) {
                toggle("Start at Login", state.startAtLogin) { model.send(.setStartAtLogin(on: $0)) }
            }
        }
    }

    @ViewBuilder
    private func toggle(_ name: String, _ isOn: Bool, _ set: @escaping (Bool) -> Void) -> some View {
        let toggle = Toggle(name, isOn: Binding(get: { isOn }, set: set)).labelsHidden()
        if offscreen {
            toggle.toggleStyle(DrawnSwitchStyle())
        } else {
            toggle.toggleStyle(.switch).controlSize(.small).tint(Brand.accent)
                // The row's tap is the one way to flip it, so a click on the switch can't fire twice.
                .allowsHitTesting(false)
        }
    }
}
