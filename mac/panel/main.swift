// sidescreen-panel: Sidescreen's menu-bar icon and panel on the Mac.
//
// The Sidescreen app starts it and talks to it over stdin and stdout (see Bridge.swift).
//
//   sidescreen-panel                        the menu-bar panel, fed by the app
//   sidescreen-panel --check-fixture F...   decodes each fixture and runs the self-test, no window
//   sidescreen-panel --open                 also shows the panel in a window at launch (tests)
//   sidescreen-panel --fixture F            takes its state from F instead of stdin (tests)

import AppKit
import SwiftUI

@main
enum Entry {
    static func main() {
        let args = Array(CommandLine.arguments.dropFirst())
        if args.first == "--check-fixture" {
            exit(MainActor.assumeIsolated { SelfTest.run(fixtures: Array(args.dropFirst())) })
        }
        if args.first == "--render" {
            exit(MainActor.assumeIsolated { Render.run(Array(args.dropFirst())) })
        }
        Launch.open = args.contains("--open")
        if let i = args.firstIndex(of: "--fixture") {
            guard i + 1 < args.count, let data = FileManager.default.contents(atPath: args[i + 1]),
                let dto = decodeState(data)
            else {
                FileHandle.standardError.write(Data("sidescreen-panel: unreadable fixture\n".utf8))
                exit(2)
            }
            Launch.fixture = dto
        }
        SidescreenPanel.main()
    }
}

/// How this run was started.
enum Launch {
    static var open = false
    static var fixture: PanelDto?
}

struct SidescreenPanel: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var delegate

    var body: some Scene {
        MenuBarExtra {
            PanelView(model: delegate.model)
        } label: {
            Image(nsImage: Icons.tray)
                .accessibilityLabel("Sidescreen")
        }
        .menuBarExtraStyle(.window)
    }
}

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    let model = PanelModel()
    private var window: NSPanel?
    private var quitting = false
    private let statusWatch = StatusItemWatch()

    func applicationWillFinishLaunching(_ notification: Notification) {
        NSApp.setActivationPolicy(.accessory)
        if let dto = Launch.fixture {
            model.apply(dto)
            // With no app on the other end, the actions are only shown, and Quit quits.
            model.output = { line in
                Bridge.write(line)
                if line == PanelAction.quit.line { exit(0) }
            }
        } else {
            PanelLog.enabled = true
            PanelLog.write("start pid=\(ProcessInfo.processInfo.processIdentifier)")
            Bridge.startReading(into: model)
        }
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        if Launch.open { showWindow() }
        // The menu-bar item appears just after launch.
        DispatchQueue.main.asyncAfter(deadline: .now() + 1) { [statusWatch] in statusWatch.start() }
    }

    /// A quit request from the system (log out, `tell application ... to quit`) quits the whole
    /// app, which then closes stdin and so ends this process. Not ending at once keeps the app
    /// from taking this as a crash and restarting the panel; after 5 s it ends anyway.
    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        if Launch.fixture != nil { return .terminateNow }
        if !quitting {
            quitting = true
            model.send(.quit)
            DispatchQueue.main.asyncAfter(deadline: .now() + 5) {
                NSApp.reply(toApplicationShouldTerminate: true)
            }
        }
        return .terminateLater
    }

    /// The panel in an ordinary window, for screenshots: the menu-bar panel can't be opened from
    /// code. Prints "window <id>" on stderr, for `screencapture -l <id>`.
    private func showWindow() {
        let panel = NSPanel(
            contentRect: NSRect(x: 0, y: 0, width: 400, height: 300),
            styleMask: [.titled, .fullSizeContentView, .nonactivatingPanel],
            backing: .buffered, defer: false)
        panel.titlebarAppearsTransparent = true
        panel.titleVisibility = .hidden
        for button in [NSWindow.ButtonType.closeButton, .miniaturizeButton, .zoomButton] {
            panel.standardWindowButton(button)?.isHidden = true
        }
        panel.isMovableByWindowBackground = true
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.title = "Sidescreen"
        let host = NSHostingController(rootView: PanelView(model: model).background(WindowMaterial()))
        host.sizingOptions = [.preferredContentSize]  // the window follows the panel's height
        panel.contentViewController = host
        let place = {
            if let screen = NSScreen.main?.visibleFrame {
                panel.setFrameTopLeftPoint(NSPoint(x: screen.maxX - panel.frame.width - 80, y: screen.maxY - 8))
            }
        }
        place()
        // Once the scroll area has measured its content, pin the top again.
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) { place() }
        panel.orderFrontRegardless()
        window = panel
        FileHandle.standardError.write(Data("window \(panel.windowNumber)\n".utf8))
    }
}

/// The menu-bar panel's material, for the `--open` window.
private struct WindowMaterial: NSViewRepresentable {
    func makeNSView(context: Context) -> NSVisualEffectView {
        let view = NSVisualEffectView()
        view.material = .popover
        view.blendingMode = .behindWindow
        view.state = .active
        return view
    }

    func updateNSView(_ view: NSVisualEffectView, context: Context) {}
}

enum Icons {
    /// The menu-bar icon: a template image, so it follows the menu bar's colour. 18 pt, with an
    /// 18 px drawing for 1x screens and a 36 px one for 2x.
    static let tray: NSImage = {
        let icon = NSImage(size: NSSize(width: 18, height: 18))
        for png in [trayIconPNG, trayIcon2xPNG] {
            if let data = Data(base64Encoded: png), let rep = NSBitmapImageRep(data: data) {
                rep.size = icon.size
                icon.addRepresentation(rep)
            }
        }
        icon.isTemplate = true
        return icon
    }()

    static let app: NSImage = image(base64: appIconPNG)

    private static func image(base64: String) -> NSImage {
        Data(base64Encoded: base64).flatMap { NSImage(data: $0) } ?? NSImage()
    }
}
