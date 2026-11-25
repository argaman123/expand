// A small log for checking the panel on a Mac without looking at it:
// ~/Library/Logs/Expand/panel.log, emptied once it passes 64 KB. It records where the
// menu-bar item is, opening and closing, and what the state lines did. Never the pairing code:
// states are logged as a summary only.

import AppKit
import Foundation

enum PanelLog {
    /// Off in the self-test, `--render` and fixture runs.
    nonisolated(unsafe) static var enabled = false
    private static let queue = DispatchQueue(label: "panel log")
    private static let limit = 64 * 1024

    static func write(_ line: String) {
        guard enabled else { return }
        let stamp = Date()
        queue.async {
            let dir = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Logs/Expand")
            let file = dir.appendingPathComponent("panel.log")
            try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
            let size = (try? FileManager.default.attributesOfItem(atPath: file.path)[.size] as? Int) ?? 0
            if size > limit || !FileManager.default.fileExists(atPath: file.path) {
                FileManager.default.createFile(atPath: file.path, contents: nil)
            }
            guard let handle = try? FileHandle(forWritingTo: file) else { return }
            defer { try? handle.close() }
            _ = try? handle.seekToEnd()
            let time = ISO8601DateFormatter.string(
                from: stamp, timeZone: .current, formatOptions: [.withFullDate, .withFullTime, .withSpaceBetweenDateAndTime])
            try? handle.write(contentsOf: Data("\(time) \(line)\n".utf8))
        }
    }

    /// A window frame as `x y w h`, in points from the top left of the main display.
    @MainActor static func topLeft(_ frame: NSRect) -> String {
        let top = (NSScreen.screens.first?.frame.maxY ?? frame.maxY) - frame.maxY
        return "\(Int(frame.minX)) \(Int(top)) \(Int(frame.width)) \(Int(frame.height))"
    }

    /// A state line as a summary, without the code.
    static func summary(_ dto: PanelDto) -> String {
        "state version=\(dto.version) connected=\(dto.connected == nil ? "no" : "yes") others=\(dto.others.count) "
            + "pairing=\(dto.pairing == nil ? "closed" : "open") missing=\(dto.missing.count)"
    }
}

/// Logs the menu-bar item's place and visibility at launch and whenever they change. Driven by
/// the item window's own notifications (moved, resized, shown or hidden, another screen) and
/// screen changes; it looks every 2 s only until that window exists, for a minute at most.
@MainActor
final class StatusItemWatch {
    private var last = ""
    private var timer: Timer?
    private weak var watched: NSWindow?
    private var windowObservers: [NSObjectProtocol] = []
    private var screenObserver: NSObjectProtocol?

    func start() {
        let started = Date()
        check()
        if watched == nil {
            timer = Timer.scheduledTimer(withTimeInterval: 2, repeats: true) { [weak self] timer in
                MainActor.assumeIsolated {
                    guard let self else { return timer.invalidate() }
                    self.check()
                    if self.watched != nil || Date().timeIntervalSince(started) > 60 {
                        timer.invalidate()
                        self.timer = nil
                    }
                }
            }
        }
        screenObserver = NotificationCenter.default.addObserver(
            forName: NSApplication.didChangeScreenParametersNotification, object: nil, queue: .main
        ) { [weak self] _ in
            MainActor.assumeIsolated { self?.check() }
        }
    }

    private func check() {
        let window = Self.statusWindow()
        if window !== watched { watch(window) }
        let line = Self.describe(window)
        if line != last {
            last = line
            PanelLog.write(line)
        }
    }

    private func watch(_ window: NSWindow?) {
        windowObservers.forEach(NotificationCenter.default.removeObserver)
        windowObservers = []
        watched = window
        guard let window else { return }
        let names = [
            NSWindow.didChangeOcclusionStateNotification, NSWindow.didMoveNotification,
            NSWindow.didResizeNotification, NSWindow.didChangeScreenNotification,
        ]
        for name in names {
            windowObservers.append(
                NotificationCenter.default.addObserver(forName: name, object: window, queue: .main) { [weak self] _ in
                    MainActor.assumeIsolated { self?.check() }
                })
        }
    }

    private static func statusWindow() -> NSWindow? {
        NSApp.windows.first { NSStringFromClass(type(of: $0)).contains("StatusBar") }
    }

    /// `status-item x y w h visible=<bool> screen=<WxH>`, in screen points from the top left of
    /// the main display.
    static func describe(_ window: NSWindow?) -> String {
        guard let window else {
            let names = Set(NSApp.windows.map { NSStringFromClass(type(of: $0)) }).sorted()
            return "status-item none windows=[\(names.joined(separator: ","))]"
        }
        let visible = window.isVisible && window.occlusionState.contains(.visible)
        let screen = (window.screen ?? NSScreen.main)?.frame.size ?? .zero
        return "status-item \(PanelLog.topLeft(window.frame)) "
            + "visible=\(visible) screen=\(Int(screen.width))x\(Int(screen.height))"
    }
}
