// Dev-only snapshots for live checks. With EXPAND_PANEL_SNAPSHOT_DIR set when the helper
// starts, the open panel's window is saved as DIR/panel-<n>.png whenever what it shows changes
// (after 400 ms of quiet), and `snapshot <path>` is logged. Without the variable, nothing.

import AppKit
import Foundation

@MainActor
final class Snapshots {
    static let shared = Snapshots()

    /// The panel window, set when it shows.
    weak var window: NSWindow?
    private let dir: URL?
    private var count = 0
    private var pending: DispatchWorkItem?

    private init() {
        guard let path = ProcessInfo.processInfo.environment["EXPAND_PANEL_SNAPSHOT_DIR"], !path.isEmpty
        else {
            dir = nil
            return
        }
        let dir = URL(fileURLWithPath: path, isDirectory: true)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let existing = (try? FileManager.default.contentsOfDirectory(atPath: dir.path)) ?? []
        count = existing.filter { $0.hasPrefix("panel-") && $0.hasSuffix(".png") }.count
        self.dir = dir
    }

    /// Takes a snapshot once things have been still for 400 ms.
    func request() {
        guard dir != nil else { return }
        pending?.cancel()
        let item = DispatchWorkItem { [weak self] in
            MainActor.assumeIsolated { self?.take() }
        }
        pending = item
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.4, execute: item)
    }

    private func take() {
        guard let dir, let window, window.isVisible, window.occlusionState.contains(.visible) else { return }
        guard let image = Self.capture(CGWindowID(window.windowNumber)),
            let png = NSBitmapImageRep(cgImage: image).representation(using: .png, properties: [:])
        else {
            PanelLog.write("snapshot failed")
            return
        }
        count += 1
        let url = dir.appendingPathComponent("panel-\(count).png")
        do {
            try png.write(to: url)
            PanelLog.write("snapshot \(url.path)")
        } catch {
            PanelLog.write("snapshot failed: \(url.path)")
        }
    }

    private typealias CreateImage = @convention(c) (CGRect, UInt32, UInt32, UInt32) -> Unmanaged<CGImage>?

    /// `CGWindowListCreateImage` of one window, looked up at run time: the SDK marks it obsolete,
    /// but it still works and needs no ScreenCaptureKit session for a dev check.
    private static func capture(_ window: CGWindowID) -> CGImage? {
        guard let symbol = dlsym(UnsafeMutableRawPointer(bitPattern: -2), "CGWindowListCreateImage") else { return nil }
        let create = unsafeBitCast(symbol, to: CreateImage.self)
        let includingWindow: UInt32 = 1 << 3
        let ignoreFramingBestResolution: UInt32 = (1 << 0) | (1 << 3)
        return create(.null, includingWindow, window, ignoreFramingBestResolution)?.takeRetainedValue()
    }
}
