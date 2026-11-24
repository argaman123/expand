// The whole panel: header, the connected PC, the other PCs with Add PC, permissions, This Mac,
// and Quit. About 400pt wide; the middle scrolls once the panel would pass about 560pt.

import AppKit
import SwiftUI

struct PanelView: View {
    let model: PanelModel
    var width: CGFloat = 400
    @Environment(\.offscreen) private var offscreen
    @State private var contentHeight: CGFloat = 0

    /// The panel's height cap, and what the header and footer take of it.
    private static let maxHeight: CGFloat = 560
    private static let chromeHeight: CGFloat = 76

    var body: some View {
        VStack(spacing: 0) {
            header
                .padding([.horizontal, .top], 16)
            if offscreen {
                sections.padding(16)  // drawn whole: a scroll view doesn't draw offscreen
            } else {
                ScrollView(.vertical) {
                    sections
                        .padding(16)
                        .background(
                            GeometryReader { geometry in
                                Color.clear.preference(key: ContentHeight.self, value: geometry.size.height)
                            })
                }
                .scrollBounceBehavior(.basedOnSize)
                .frame(height: min(contentHeight, scrollCap))
                .onPreferenceChange(ContentHeight.self) { height in
                    MainActor.assumeIsolated { contentHeight = height }
                }
            }
            footer
                .padding([.horizontal, .bottom], 16)
        }
        .frame(width: width)
        .background { BrandWash(shown: model.isOpen) }
        .background {
            if !offscreen {
                WindowVisibility { visible in
                    if visible { model.panelOpened() } else { model.panelClosed() }
                }
            }
        }
        .onChange(of: SnapshotKey(model: model)) { Snapshots.shared.request() }
        .onAppear { model.panelOpened() }
        .onDisappear { model.panelClosed() }
    }

    private var scrollCap: CGFloat {
        let screen = NSScreen.main?.visibleFrame.height ?? Self.maxHeight
        return max(160, min(Self.maxHeight, screen - 40) - Self.chromeHeight)
    }

    private var header: some View {
        HStack(spacing: 10) {
            Image(nsImage: Icons.app)
                .resizable()
                .frame(width: 24, height: 24)
                .accessibilityHidden(true)
            Text("Sidescreen")
                .font(.system(size: 15, weight: .semibold))
            Spacer(minLength: 8)
            if let version = model.state?.version {
                Text(version)
                    .font(.system(size: 11))
                    .foregroundStyle(.tertiary)
                    .monospacedDigit()
            }
        }
        .padding(.horizontal, 4)
    }

    @ViewBuilder private var sections: some View {
        if let state = model.state {
            VStack(alignment: .leading, spacing: 16) {
                if let pc = model.shownConnected {
                    ConnectedSection(pc: pc, model: model, restarting: model.restarting != nil)
                }
                PanelSection(title: pcsTitle) {
                    ForEach(model.shownOthers) { pc in
                        PcRow(pc: pc, model: model, anyConnected: model.shownConnected != nil)
                        BoxDivider()
                    }
                    PairingView(model: model)
                }
                PermissionsSection(state: state, model: model)
                ThisMacSection(state: state, model: model)
            }
        }
    }

    /// "Devices" with nothing connected; "Other devices" only when there are other devices, so a lone Add device
    /// under the connected PC has no heading.
    private var pcsTitle: String? {
        if model.shownConnected == nil { return "Devices" }
        return model.shownOthers.isEmpty ? nil : "Other devices"
    }

    private var footer: some View {
        HStack {
            Spacer()
            QuitButton { model.send(.quit) }
        }
        .padding(.horizontal, 6)
    }
}

/// What the panel shows, for knowing when to take a dev snapshot.
private struct SnapshotKey: Equatable {
    let state: PanelDto?
    let connecting: String?
    let restarting: String?
    let forget: String?

    @MainActor init(model: PanelModel) {
        state = model.state
        connecting = model.connectingKey
        restarting = model.restarting?.key
        forget = model.confirmingForget
    }
}

private struct ContentHeight: PreferenceKey {
    static let defaultValue: CGFloat = 0
    static func reduce(value: inout CGFloat, nextValue: () -> CGFloat) { value = max(value, nextValue()) }
}

private struct QuitButton: View {
    let action: () -> Void
    @State private var hovering = false
    @Environment(\.previewInteraction) private var preview
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        Button(action: action) {
            Text("Quit Sidescreen")
                .font(.system(size: 12))
                .foregroundStyle(lit ? .primary : .secondary)
                .padding(.horizontal, 10)
                .frame(height: 24)
                .background(Color.primary.opacity(lit ? 0.08 : 0), in: Capsule())
                .contentShape(Capsule())
        }
        .buttonStyle(PressStyle(shape: Capsule(), depth: 0.08, scale: 0.97))
        .onHover { hovering = $0 }
        .pointerCursor()
        .animation(reduceMotion ? nil : .easeOut(duration: 0.12), value: lit)
    }

    private var lit: Bool { hovering || preview != nil }
}

/// A plain button's press: a dark wash over its shape and a small dip. `--render-pressed`
/// shows it without a mouse.
struct PressStyle<S: Shape>: ButtonStyle {
    let shape: S
    var depth = 0.08
    var scale = 0.97
    @Environment(\.previewInteraction) private var preview

    func makeBody(configuration: Configuration) -> some View {
        let pressed = configuration.isPressed || preview == .pressed
        configuration.label
            .overlay(shape.fill(Color.primary.opacity(pressed ? depth : 0)))
            .scaleEffect(pressed ? scale : 1)
    }
}

// MARK: - Sections and boxes

/// A titled group: a small heading over one rounded box of lines.
struct PanelSection<Content: View>: View {
    /// Nil for a box without a heading.
    let title: String?
    @ViewBuilder let content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            if let title {
                Text(title)
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundStyle(.primary.opacity(0.85))
                    .padding(.horizontal, 6)
            }
            VStack(spacing: 0) { content }
                .background(Color.primary.opacity(0.06), in: RoundedRectangle(cornerRadius: 12, style: .continuous))
                .overlay(
                    RoundedRectangle(cornerRadius: 12, style: .continuous)
                        .strokeBorder(Color.primary.opacity(0.06), lineWidth: 0.5))
        }
    }
}

struct BoxDivider: View {
    var body: some View {
        Rectangle()
            .fill(Color.primary.opacity(0.08))
            .frame(height: 0.5)
    }
}

/// A plain line in a box: label, then whatever goes on the right.
struct BoxLine<Trailing: View>: View {
    var warning = false
    /// Keeps the dot's room even without a dot, so labels line up in a box that has dots.
    var dotRoom = false
    let label: String
    /// A line that does something when clicked anywhere (a switch's line) lights up on hover.
    var action: (() -> Void)? = nil
    @ViewBuilder let trailing: Trailing
    @State private var hovering = false

    var body: some View {
        line
            .rowHighlight(action != nil && hovering, preview: action != nil)
            .contentShape(Rectangle())
            .onHover { hovering = $0 }
            .onTapGesture { action?() }
            .pointerCursor(action != nil)
    }

    private var line: some View {
        HStack(spacing: 10) {
            if warning {
                OrangeDot()
            } else if dotRoom {
                Color.clear.frame(width: 8, height: 8)
            }
            Text(verbatim: label)
                .font(.system(size: 13))
                .frame(maxWidth: .infinity, alignment: .leading)
            trailing
        }
        .frame(minHeight: 42)
        .padding(.horizontal, 12)
    }
}

struct OrangeDot: View {
    var body: some View {
        Circle()
            .fill(Color(nsColor: .systemOrange))
            .frame(width: 8, height: 8)
            .accessibilityHidden(true)
    }
}

// MARK: - Buttons

/// The panel's small buttons: grey (Allow, Cancel), the brand's gradient (Connect) or red (Forget).
struct PillButtonStyle: ButtonStyle {
    enum Kind { case plain, brand, red }
    var kind: Kind = .plain

    func makeBody(configuration: Configuration) -> some View {
        Pill(configuration: configuration, kind: kind)
    }

    private struct Pill: View {
        let configuration: ButtonStyleConfiguration
        let kind: Kind
        @State private var hovering = false
        @Environment(\.previewInteraction) private var preview
        @Environment(\.accessibilityReduceMotion) private var reduceMotion

        private var lit: Bool { hovering || preview != nil }
        private var pressed: Bool { configuration.isPressed || preview == .pressed }

        var body: some View {
            configuration.label
                .font(.system(size: 12, weight: kind == .plain ? .medium : .semibold))
                .foregroundStyle(kind == .plain ? Color.primary : Color.white)
                .lineLimit(1)
                .padding(.horizontal, kind == .plain ? 12 : 14)
                .frame(height: 26)
                .background(fill, in: shape)
                .contentShape(shape)
                .brightness(kind == .plain ? 0 : (pressed ? -0.12 : (lit ? hoverLift : 0)))
                .scaleEffect(pressed ? 0.96 : 1)
                .onHover { hovering = $0 }
                .pointerCursor()
                .animation(reduceMotion ? nil : .easeOut(duration: 0.12), value: lit)
                .animation(reduceMotion ? nil : .easeOut(duration: 0.08), value: pressed)
        }

        /// The hover's lift; small on the brand's gradient so Connect's label keeps its contrast.
        private var hoverLift: Double { kind == .brand ? 0.04 : 0.08 }

        private var shape: RoundedRectangle {
            RoundedRectangle(cornerRadius: kind == .plain ? 7 : 13, style: .continuous)
        }

        private var fill: AnyShapeStyle {
            switch kind {
            case .plain: return AnyShapeStyle(Color.primary.opacity(pressed ? 0.3 : (lit ? 0.2 : 0.12)))
            case .brand: return AnyShapeStyle(Brand.control)
            case .red: return AnyShapeStyle(Color(nsColor: .systemRed))
            }
        }
    }
}

// MARK: - Open and close

/// Reports when the hosting window is shown or hidden. The menu-bar panel is ordered out rather
/// than closed, so this backs up `onAppear` and `onDisappear`. Also logs `panel open x y w h`
/// when it shows, and again when it moves or resizes while shown.
private struct WindowVisibility: NSViewRepresentable {
    let changed: (Bool) -> Void

    func makeNSView(context: Context) -> VisibilityView {
        let view = VisibilityView()
        view.changed = changed
        return view
    }

    func updateNSView(_ view: VisibilityView, context: Context) {
        view.changed = changed
    }

    final class VisibilityView: NSView {
        var changed: ((Bool) -> Void)?
        private var observers: [NSObjectProtocol] = []
        /// The frame last logged while shown; nil while hidden.
        private var loggedFrame: String?

        override func viewDidMoveToWindow() {
            super.viewDidMoveToWindow()
            observers.forEach(NotificationCenter.default.removeObserver)
            observers = []
            loggedFrame = nil
            guard let window else { return }
            let center = NotificationCenter.default
            observers.append(
                center.addObserver(forName: NSWindow.didChangeOcclusionStateNotification, object: window, queue: .main) {
                    [weak self] _ in
                    MainActor.assumeIsolated {
                        guard let self, let window = self.window else { return }
                        let visible = window.occlusionState.contains(.visible)
                        self.changed?(visible)
                        self.logFrame(visible: visible)
                    }
                })
            for name in [NSWindow.didResizeNotification, NSWindow.didMoveNotification] {
                observers.append(
                    center.addObserver(forName: name, object: window, queue: .main) { [weak self] _ in
                        MainActor.assumeIsolated {
                            guard let self, let window = self.window else { return }
                            self.logFrame(visible: window.isVisible && window.occlusionState.contains(.visible))
                        }
                    })
            }
            logFrame(visible: window.isVisible && window.occlusionState.contains(.visible))
        }

        private func logFrame(visible: Bool) {
            guard visible, let window else {
                loggedFrame = nil
                return
            }
            let frame = PanelLog.topLeft(window.frame)
            if frame != loggedFrame {
                loggedFrame = frame
                PanelLog.write("panel open \(frame)")
                Snapshots.shared.window = window
                Snapshots.shared.request()
            }
        }

        deinit {
            observers.forEach(NotificationCenter.default.removeObserver)
        }
    }
}
