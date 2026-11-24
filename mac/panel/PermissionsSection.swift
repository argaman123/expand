// Shown only when something is missing: Screen Recording and Accessibility, with an orange dot
// and Allow on the missing one, and any streaming app this Mac lacks, with Install.

import SwiftUI

struct PermissionsSection: View {
    let state: PanelDto
    let model: PanelModel

    var body: some View {
        let permissions = state.permissions
        if permissions.screen == false || permissions.accessibility == false || !state.missing.isEmpty {
            PanelSection(title: "Permissions") {
                permission("Screen Recording", permissions.screen, .screen)
                BoxDivider()
                permission("Accessibility", permissions.accessibility, .accessibility)
                ForEach(state.missing, id: \.self) { name in
                    BoxDivider()
                    BoxLine(warning: true, label: name) {
                        Button("Install") { model.send(.installMissing) }
                            .buttonStyle(PillButtonStyle())
                    }
                }
            }
        }
    }

    /// `granted` is nil when the app couldn't tell.
    private func permission(_ name: String, _ granted: Bool?, _ kind: PermissionKind) -> some View {
        BoxLine(warning: granted == false, dotRoom: true, label: name) {
            if granted == true {
                Image(systemName: "checkmark")
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundStyle(Color(nsColor: .systemGreen))
                    .accessibilityLabel("Allowed")
            } else if granted == false {
                Button("Allow") { model.send(.allowPermission(kind: kind)) }
                    .buttonStyle(PillButtonStyle())
            }
        }
    }
}
