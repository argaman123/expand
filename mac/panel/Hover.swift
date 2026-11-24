// Hover and pressed looks shared by the panel's controls: a pointing-hand cursor over anything
// that acts, a soft inset highlight on rows in the brand's accent, and a render-only way to show
// the states (`--render-hover`, `--render-pressed`), since hover can't happen offscreen.

import AppKit
import SwiftUI

/// What `--render` pretends every control is doing.
enum PreviewInteraction {
    case hover, pressed
}

private struct PreviewInteractionKey: EnvironmentKey {
    static let defaultValue: PreviewInteraction? = nil
}

extension EnvironmentValues {
    var previewInteraction: PreviewInteraction? {
        get { self[PreviewInteractionKey.self] }
        set { self[PreviewInteractionKey.self] = newValue }
    }
}

extension View {
    /// The pointing hand over a control that acts, while it is enabled.
    func pointerCursor(_ on: Bool = true) -> some View { modifier(PointerCursor(active: on)) }

    /// A soft rounded highlight inset in a box row, like a menu item's.
    func rowHighlight(_ on: Bool, preview: Bool = true) -> some View {
        modifier(RowHighlight(on: on, previewable: preview))
    }
}

private struct PointerCursor: ViewModifier {
    let active: Bool
    @Environment(\.isEnabled) private var enabled
    @State private var inside = false

    func body(content: Content) -> some View {
        content
            .onHover { hovering in
                if hovering && enabled && active && !inside {
                    NSCursor.pointingHand.push()
                    inside = true
                } else if (!hovering || !enabled || !active) && inside {
                    NSCursor.pop()
                    inside = false
                }
            }
            .onDisappear {
                if inside {
                    NSCursor.pop()
                    inside = false
                }
            }
    }
}

private struct RowHighlight: ViewModifier {
    let on: Bool
    let previewable: Bool
    @Environment(\.previewInteraction) private var preview
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    func body(content: Content) -> some View {
        let shown = on || (previewable && preview != nil)
        content
            .background {
                RoundedRectangle(cornerRadius: 8, style: .continuous)
                    .fill(Brand.accent.opacity(shown ? 0.13 : 0))
                    .padding(.horizontal, 4)
                    .padding(.vertical, 3)
            }
            .animation(reduceMotion ? nil : .easeOut(duration: 0.12), value: shown)
    }
}
