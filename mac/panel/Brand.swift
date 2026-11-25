// Expand's colours, "Shore" (round 3), from the app icon (design/icons/icon.svg): its sky blue
// to violet gradient (#3aa8ff, #5d8bff at 55%, #8a5cf6; also app/src/brand.ts and brand.css), an
// accent between the first two for switches and hovers, a solid blue for Connect, and the faint wash of them behind
// the panel. The names are the old ones: `blue` is the start, `violet` the middle, `magenta` the end.

import SwiftUI

enum Brand {
    static let blue = Color(red: 0x3a / 255, green: 0xa8 / 255, blue: 0xff / 255)  // #3aa8ff
    static let violet = Color(red: 0x5d / 255, green: 0x8b / 255, blue: 0xff / 255)  // #5d8bff
    static let magenta = Color(red: 0x8a / 255, green: 0x5c / 255, blue: 0xf6 / 255)  // #8a5cf6
    /// The accent, for switches and hovers: 40% of the way from the start to the middle.
    static let accent = Color(red: 0x48 / 255, green: 0x9c / 255, blue: 0xff / 255)  // #489cff

    /// A small filled control (Connect): one solid blue, as what is on in the PC's window
    /// (`BRAND_ON` in app/src/brand.ts), never the gradient. White 12 pt text on it is 4.7:1;
    /// hovered it dims (5.1:1 and up) rather than lighten.
    static let control = Color(red: 0x4f / 255, green: 0x6b / 255, blue: 0xdb / 255)  // #4f6bdb
}

/// Behind the whole panel: a faint blue glow from the top left and a fainter violet one from the
/// bottom right, drifting slowly (one round a minute) while the panel is shown. Still with Reduce
/// Motion; fainter and still with Reduce Transparency, where the panel is solid.
struct BrandWash: View {
    /// The panel is on screen; the drift stops while it is closed.
    var shown = true
    @Environment(\.colorScheme) private var scheme
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Environment(\.accessibilityReduceTransparency) private var reduceTransparency
    @Environment(\.offscreen) private var offscreen
    private static let period = 60.0

    var body: some View {
        let still = !shown || reduceMotion || reduceTransparency || offscreen
        TimelineView(.animation(minimumInterval: 1.0 / 20, paused: still)) { context in
            let turn = still
                ? 0
                : context.date.timeIntervalSinceReferenceDate.truncatingRemainder(dividingBy: Self.period)
                    / Self.period * 2 * .pi
            GeometryReader { geometry in
                let reach = max(geometry.size.width, geometry.size.height)
                ZStack {
                    RadialGradient(
                        colors: [Brand.blue.opacity(strength.blue), Brand.blue.opacity(0)],
                        center: UnitPoint(x: 0.1 + 0.08 * cos(turn), y: 0.02 + 0.05 * sin(turn)),
                        startRadius: 0, endRadius: reach * 0.75)
                    RadialGradient(
                        colors: [Brand.violet.opacity(strength.violet), Brand.magenta.opacity(0)],
                        center: UnitPoint(x: 0.92 - 0.08 * cos(turn), y: 0.98 - 0.05 * sin(turn)),
                        startRadius: 0, endRadius: reach * 0.7)
                }
            }
        }
        .allowsHitTesting(false)
        .accessibilityHidden(true)
    }

    private var strength: (blue: Double, violet: Double) {
        let scale = reduceTransparency ? 0.6 : 1
        return scheme == .dark ? (0.16 * scale, 0.12 * scale) : (0.10 * scale, 0.08 * scale)
    }
}
