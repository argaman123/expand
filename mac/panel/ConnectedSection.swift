// The connected PC on a slowly turning blue-to-violet gradient (the icon's, Brand.swift): the PC in a frosted circle, its
// name, its address (the round trip in the address's tooltip), what's in use, then Disconnect
// and restart.

import AppKit
import SwiftUI

struct ConnectedSection: View {
    let pc: PanelPc
    let model: PanelModel
    /// A Restart is under way: the section stays, with a spinner on the restart button.
    var restarting = false
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Environment(\.offscreen) private var offscreen

    private let shape = RoundedRectangle(cornerRadius: 16, style: .continuous)

    var body: some View {
        VStack(spacing: 18) {
            HStack(spacing: 14) {
                Image(systemName: "desktopcomputer")
                    .font(.system(size: 21, weight: .regular))
                    .frame(width: 48, height: 48)
                    .frosted(Circle())
                    .accessibilityHidden(true)
                VStack(alignment: .leading, spacing: 3) {
                    Text(verbatim: pc.name)
                        .font(.system(size: 17, weight: .semibold))
                        .lineLimit(1)
                    detail
                }
                Spacer(minLength: 8)
                uses
            }
            HStack(spacing: 8) {
                Button("Disconnect") { model.send(.disconnect) }
                    .buttonStyle(GlassButtonStyle())
                Button {
                    model.send(.restart)
                } label: {
                    if restarting {
                        Spinner(size: 13)
                    } else {
                        Image(systemName: "arrow.clockwise")
                            .font(.system(size: 13, weight: .semibold))
                    }
                }
                .buttonStyle(GlassButtonStyle(square: true))
                .disabled(restarting)
                .help("Restart connection")
                .accessibilityLabel("Restart connection")
            }
        }
        .padding(18)
        .foregroundStyle(.white)
        .environment(\.colorScheme, .dark)
        .background(LiveGradient(moving: !reduceMotion && (model.isOpen || offscreen)))
        .clipShape(shape)
        .overlay(shape.strokeBorder(.white.opacity(0.25), lineWidth: 0.5))
        .shadow(color: Brand.violet.opacity(0.35), radius: 15, y: 10)
    }

    @ViewBuilder private var detail: some View {
        let text = Text(verbatim: restarting ? "Restarting" : pc.connectedDetail)
            .font(.system(size: 12))
            .foregroundStyle(.white.opacity(0.9))
            .monospacedDigit()
            .lineLimit(1)
        if let help = pc.roundTripHelp { text.help(help) } else { text }
    }

    /// Only what is in use; the names are in the tooltips.
    private var uses: some View {
        HStack(spacing: 12) {
            if let uses = pc.uses {
                if uses.mouse { use("computermouse", "Mouse and keyboard") }
                if uses.clipboard { use("doc.on.clipboard", "Clipboard") }
                if uses.display { use("display", pc.display.map { "Display \($0)" } ?? "Display") }
            }
        }
    }

    private func use(_ symbol: String, _ name: String) -> some View {
        Image(systemName: symbol)
            .font(.system(size: 15))
            .frame(height: 20)
            .help(name)
            .accessibilityLabel(name)
    }
}

/// The soft gradient behind the connected PC, one turn every 14 s; still with Reduce Motion, when
/// it lies as on the icon: blue at the top left, the magenta at the bottom right.
private struct LiveGradient: View {
    let moving: Bool
    private static let period = 14.0
    /// The icon's stops round the circle, back to the blue.
    private static let stops: [Gradient.Stop] = [
        .init(color: Brand.blue, location: 0), .init(color: Brand.violet, location: 0.3),
        .init(color: Brand.magenta, location: 0.55), .init(color: Brand.blue, location: 1),
    ]

    var body: some View {
        TimelineView(.animation(minimumInterval: 1.0 / 30, paused: !moving)) { context in
            let turn = moving
                ? context.date.timeIntervalSinceReferenceDate.truncatingRemainder(dividingBy: Self.period)
                    / Self.period
                : 0
            GeometryReader { geometry in
                let side = max(geometry.size.width, geometry.size.height) * 1.8
                AngularGradient(
                    stops: Self.stops, center: .center, startAngle: .degrees(225), endAngle: .degrees(585))
                    .frame(width: side, height: side)
                    .rotationEffect(.degrees(turn * 360))
                    .blur(radius: 40)
                    .position(x: geometry.size.width / 2, y: geometry.size.height / 2)
            }
        }
        .opacity(0.85)
        .background(Color(red: 0.11, green: 0.13, blue: 0.24))
        // Deep enough that the 90% white detail text keeps 4.5:1 over the blue (4.8:1 at worst).
        .overlay(Color(red: 0.08, green: 0.09, blue: 0.16).opacity(0.2))
        .accessibilityHidden(true)
    }
}

/// The wide Disconnect button and the round restart button: frosted, on the gradient.
private struct GlassButtonStyle: ButtonStyle {
    var square = false
    @State private var hovering = false
    @Environment(\.previewInteraction) private var preview
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    func makeBody(configuration: Configuration) -> some View {
        let pressed = configuration.isPressed || preview == .pressed
        let lit = hovering || preview != nil
        return configuration.label
            .font(.system(size: 13, weight: .semibold))
            .foregroundStyle(.white)
            .frame(maxWidth: square ? 36 : .infinity)
            .frame(width: square ? 36 : nil, height: 36)
            .frosted(Capsule(), interactive: true)
            .contentShape(Capsule())
            .brightness(pressed ? -0.06 : (lit ? 0.1 : 0))
            .scaleEffect(pressed ? 0.97 : 1)
            .onHover { hovering = $0 }
            .pointerCursor()
            .animation(reduceMotion ? nil : .easeOut(duration: 0.12), value: lit)
            .animation(reduceMotion ? nil : .easeOut(duration: 0.08), value: pressed)
    }
}

extension View {
    /// Glass for the few frosted things on the gradient: Liquid Glass where the system has it,
    /// otherwise translucent white with a light top edge.
    func frosted<S: Shape>(_ shape: S, interactive: Bool = false) -> some View {
        modifier(Frosted(shape: shape, interactive: interactive))
    }
}

private struct Frosted<S: Shape>: ViewModifier {
    let shape: S
    let interactive: Bool
    @Environment(\.offscreen) private var offscreen

    func body(content: Content) -> some View {
        if #available(macOS 26, *), !offscreen {
            content.glassEffect(interactive ? .regular.interactive() : .regular, in: shape)
        } else {
            content.modifier(FrostedFallback(shape: shape, interactive: interactive))
        }
    }
}

private struct FrostedFallback<S: Shape>: ViewModifier {
    let shape: S
    let interactive: Bool
    @State private var hovering = false
    @Environment(\.previewInteraction) private var preview

    func body(content: Content) -> some View {
        content
            .background(.white.opacity(interactive && (hovering || preview != nil) ? 0.3 : 0.18), in: shape)
            .overlay(
                shape.stroke(
                    LinearGradient(
                        colors: [.white.opacity(0.45), .white.opacity(0.05)], startPoint: .top,
                        endPoint: .bottom), lineWidth: 0.5)
            )
            .shadow(color: .black.opacity(0.12), radius: 4, y: 2)
            .onHover { if interactive { hovering = $0 } }
    }
}

extension PanelPc {
    /// Under the connected PC's name: its address, or Connecting. No ms here: a number that
    /// changes all the time says nothing (owner, 2026-11-23).
    var connectedDetail: String { isConnecting ? "Connecting" : address }

    /// The address's tooltip, once the round trip is known and while something is in use (with
    /// nothing in use the PC stops measuring, so any number would be old).
    var roundTripHelp: String? {
        guard let ms, let uses, uses.mouse || uses.clipboard || uses.display else { return nil }
        return "Round trip · \(ms) ms"
    }
}
