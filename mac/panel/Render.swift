// `sidescreen-panel --render FIXTURE OUT.png [--width 400] [--appearance dark|light]`: draws the
// panel for a fixture into a PNG at 2× without any window, then exits. Works over SSH.
// Materials, Liquid Glass and AppKit controls don't draw offscreen, so the panel's background is
// a representative solid and the switches are drawn shapes.

import AppKit
import SwiftUI

@MainActor
enum Render {
    static func run(_ args: [String]) -> Int32 {
        var rest = args
        var width: CGFloat = 400
        var scheme = ColorScheme.dark
        if let i = rest.firstIndex(of: "--width"), i + 1 < rest.count, let w = Double(rest[i + 1]) {
            width = CGFloat(w)
            rest.removeSubrange(i...(i + 1))
        }
        if let i = rest.firstIndex(of: "--appearance"), i + 1 < rest.count {
            scheme = rest[i + 1] == "light" ? .light : .dark
            rest.removeSubrange(i...(i + 1))
        }
        var preview: PreviewInteraction?
        if let i = rest.firstIndex(of: "--render-hover") {
            preview = .hover
            rest.remove(at: i)
        }
        if let i = rest.firstIndex(of: "--render-pressed") {
            preview = .pressed
            rest.remove(at: i)
        }
        guard rest.count == 2 else {
            return fail("usage: sidescreen-panel --render FIXTURE OUT.png [--width 400] [--appearance dark|light] [--render-hover|--render-pressed]")
        }
        guard let data = FileManager.default.contents(atPath: rest[0]), let dto = decodeState(data) else {
            return fail("\(rest[0]): does not decode")
        }
        let model = PanelModel()
        model.output = { _ in }
        model.apply(dto)

        let panel = PanelView(model: model, width: width)
            .background(
                RoundedRectangle(cornerRadius: 16, style: .continuous)
                    .fill(scheme == .dark ? Color(white: 0.16) : Color(white: 0.93)))
            .clipShape(RoundedRectangle(cornerRadius: 16, style: .continuous))
            .environment(\.offscreen, true)
            .environment(\.previewInteraction, preview)
            .environment(\.colorScheme, scheme)
        let renderer = ImageRenderer(content: panel)
        renderer.scale = 2
        renderer.proposedSize = ProposedViewSize(width: width, height: nil)
        guard let image = renderer.cgImage,
            let png = NSBitmapImageRep(cgImage: image).representation(using: .png, properties: [:])
        else { return fail("could not render") }
        do {
            try png.write(to: URL(fileURLWithPath: rest[1]))
        } catch {
            return fail("\(rest[1]): \(error.localizedDescription)")
        }
        FileHandle.standardOutput.write(Data("rendered \(rest[1]) \(image.width)x\(image.height)\n".utf8))
        return 0
    }

    private static func fail(_ message: String) -> Int32 {
        FileHandle.standardError.write(Data("sidescreen-panel: \(message)\n".utf8))
        return 2
    }
}

private struct OffscreenKey: EnvironmentKey {
    static let defaultValue = false
}

extension EnvironmentValues {
    /// True when drawing with `--render`: no AppKit views, materials or scrolling.
    var offscreen: Bool {
        get { self[OffscreenKey.self] }
        set { self[OffscreenKey.self] = newValue }
    }
}

/// A drawn switch, like the system's small one, for `--render`.
struct DrawnSwitchStyle: ToggleStyle {
    func makeBody(configuration: Configuration) -> some View {
        Capsule()
            .fill(configuration.isOn ? Brand.accent : Color.primary.opacity(0.18))
            .frame(width: 32, height: 18)
            .overlay(alignment: configuration.isOn ? .trailing : .leading) {
                Circle()
                    .fill(.white)
                    .shadow(color: .black.opacity(0.25), radius: 1, y: 0.5)
                    .padding(2)
            }
    }
}
