// A PC that isn't connected: icon, name and raw state. With nothing connected an allowed PC has
// a Connect button in the brand's solid blue. The ⋯ button shows on hover, on keyboard focus and while its menu is
// open; the menu holds Connect while another PC is connected, Block or Allow, and Forget…, which
// asks inline.

import AppKit
import SwiftUI

struct PcRow: View {
    let pc: PanelPc
    let model: PanelModel
    let anyConnected: Bool
    @State private var hovering = false
    @State private var menuOpen = false
    @State private var anchor = MenuAnchor()
    @FocusState private var moreFocused: Bool
    @Environment(\.offscreen) private var offscreen
    @Environment(\.previewInteraction) private var preview

    var body: some View {
        HStack(spacing: 12) {
            Image(systemName: "desktopcomputer")
                .font(.system(size: 14))
                .frame(width: 32, height: 32)
                .background(Color.primary.opacity(0.1), in: Circle())
                .opacity(pc.isBlocked ? 0.5 : 1)
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 3) {
                Text(verbatim: pc.name)
                    .font(.system(size: 13, weight: .semibold))
                    .lineLimit(1)
                    .opacity(pc.isBlocked ? 0.5 : 1)
                Text(verbatim: status)
                    .font(.system(size: 11))
                    .foregroundStyle(pc.isBlocked ? .tertiary : .secondary)
                    .monospacedDigit()
                    .lineLimit(1)
            }
            Spacer(minLength: 8)
            if model.confirmingForget == pc.key {
                Button("Forget") { model.send(.forget(key: pc.key)) }
                    .buttonStyle(PillButtonStyle(kind: .red))
                Button("Cancel") { model.cancelForget() }
                    .buttonStyle(PillButtonStyle())
            } else {
                // The ⋯ takes room only while shown, so Connect sits flush right otherwise.
                HStack(spacing: showMore ? 8 : 0) {
                    if !anyConnected && !pc.isBlocked {
                        Button {
                            model.send(.connect(key: pc.key))
                        } label: {
                            // Same size either way, so nothing moves while connecting.
                            ZStack {
                                Text("Connect").opacity(connecting ? 0 : 1)
                                if connecting { Spinner() }
                            }
                        }
                        .buttonStyle(PillButtonStyle(kind: .brand))
                    }
                    more
                }
                .animation(.easeOut(duration: 0.12), value: showMore)
            }
        }
        .frame(height: 52)
        .padding(.horizontal, 12)
        .rowHighlight(hovering || menuOpen)
        .contentShape(Rectangle())
        .onHover { hovering = $0 }
    }

    private var showMore: Bool { hovering || menuOpen || moreFocused || preview != nil }

    private var connecting: Bool { pc.isConnecting || model.connectingKey == pc.key }

    private var status: String {
        if pc.isBlocked { return "Blocked" }
        if connecting { return "Connecting" }
        switch pc.state {
        case "ready": return "\(pc.address) · Ready"
        case "connected": return "\(pc.address) · Connected"
        default: return pc.address
        }
    }

    private var more: some View {
        Button(action: showMenu) {
            Image(systemName: "ellipsis")
                .font(.system(size: 13, weight: .semibold))
                .frame(width: 26, height: 26)
                .contentShape(Circle())
        }
        .buttonStyle(MoreButtonStyle(menuOpen: menuOpen))
        .focused($moreFocused)
        .background { if !offscreen { MenuAnchorView(anchor: anchor) } }
        .frame(width: showMore ? 26 : 0)
        .clipped()
        .allowsHitTesting(showMore)
        .opacity(showMore ? 1 : 0)
        .pointerCursor()
        .help("More")
        .accessibilityLabel(Text(verbatim: "More for \(pc.name)"))
    }

    /// The ⋯ menu, right-aligned under the button. Its actions name the PC by key; one whose PC
    /// has gone meanwhile is dropped by the model.
    private func showMenu() {
        guard let view = anchor.view else { return }
        let key = pc.key
        let menu = NSMenu()
        menu.autoenablesItems = false
        let items = MenuItems()
        if anyConnected && !pc.isBlocked {
            items.add("Connect", to: menu) { model.send(.connect(key: key)) }
        }
        if pc.isBlocked {
            items.add("Allow", to: menu) { model.send(.allow(key: key)) }
        } else {
            items.add("Block", to: menu) { model.send(.block(key: key)) }
        }
        menu.addItem(.separator())
        items.add("Forget…", to: menu) { model.askForget(key) }
        menuOpen = true
        // Runs the menu until it closes, from the run loop rather than a main-queue block: the
        // main queue keeps running meanwhile, so new states still reach the panel.
        RunLoop.main.perform(inModes: [.common]) {
            MainActor.assumeIsolated {
                menu.popUp(positioning: nil, at: NSPoint(x: view.bounds.width - menu.size.width, y: -4), in: view)
                withExtendedLifetime(items) {}
                menuOpen = false
            }
        }
    }
}

/// Holds the ⋯ menu's actions (menu items keep their target weakly).
@MainActor
private final class MenuItems: NSObject {
    private var actions: [() -> Void] = []

    func add(_ title: String, to menu: NSMenu, _ action: @escaping () -> Void) {
        let item = NSMenuItem(title: title, action: #selector(run(_:)), keyEquivalent: "")
        item.target = self
        item.tag = actions.count
        actions.append(action)
        menu.addItem(item)
    }

    @objc private func run(_ item: NSMenuItem) {
        actions[item.tag]()
    }
}

/// The ⋯ button's AppKit view, so the menu can open under it.
private final class MenuAnchor {
    weak var view: NSView?
}

private struct MenuAnchorView: NSViewRepresentable {
    let anchor: MenuAnchor

    func makeNSView(context: Context) -> NSView {
        let view = NSView()
        anchor.view = view
        return view
    }

    func updateNSView(_ view: NSView, context: Context) {
        anchor.view = view
    }
}

/// A small turning ring, for a Connect in progress; a still ring with Reduce Motion.
struct Spinner: View {
    var size: CGFloat = 10
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var turning = false

    var body: some View {
        Circle()
            .trim(from: 0.25, to: 1)
            .stroke(.white, style: StrokeStyle(lineWidth: 2, lineCap: .round))
            .frame(width: size, height: size)
            .rotationEffect(.degrees(turning && !reduceMotion ? 360 : 0))
            .animation(
                reduceMotion ? nil : .linear(duration: 0.8).repeatForever(autoreverses: false),
                value: turning)
            .onAppear { turning = true }
            .accessibilityHidden(true)
    }
}

/// The ⋯ button: a faint disc that deepens on hover, while its menu is open and when pressed.
private struct MoreButtonStyle: ButtonStyle {
    let menuOpen: Bool
    @State private var hovering = false
    @Environment(\.previewInteraction) private var preview
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    func makeBody(configuration: Configuration) -> some View {
        let pressed = configuration.isPressed || preview == .pressed
        let lit = hovering || menuOpen || preview != nil
        configuration.label
            .background(Color.primary.opacity(pressed ? 0.32 : (lit ? 0.22 : 0.12)), in: Circle())
            .scaleEffect(pressed ? 0.94 : 1)
            .onHover { hovering = $0 }
            .animation(reduceMotion ? nil : .easeOut(duration: 0.12), value: lit)
    }
}
