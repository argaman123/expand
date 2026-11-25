// expand-display WIDTH HEIGHT FPS
//
// Creates Expand's virtual display at exactly that size and refresh rate, prints
// "ready <display id>", and keeps it until the Expand app goes away (its stdin closes) or
// this process is stopped. The display disappears with the process.

import CoreGraphics
import Foundation

let args = CommandLine.arguments
guard args.count == 4, let width = UInt32(args[1]), let height = UInt32(args[2]), let fps = Double(args[3]),
    width >= 320, height >= 240, fps >= 10
else {
    FileHandle.standardError.write("usage: expand-display WIDTH HEIGHT FPS\n".data(using: .utf8)!)
    exit(2)
}

let descriptor = CGVirtualDisplayDescriptor()
descriptor.queue = DispatchQueue.main
descriptor.name = "Expand"
descriptor.maxPixelsWide = width
descriptor.maxPixelsHigh = height
// A physical size like a desktop monitor's (about 110 pixels per inch), so macOS picks normal
// text sizes for it.
let millimetresPerPixel = 25.4 / 110.0
descriptor.sizeInMillimeters = CGSize(
    width: Double(width) * millimetresPerPixel, height: Double(height) * millimetresPerPixel)
descriptor.vendorID = 0x5353  // "SS"
descriptor.productID = 0x0001
descriptor.serialNum = 0x0001
descriptor.terminationHandler = { _, _ in exit(0) }

guard let display = CGVirtualDisplay(descriptor: descriptor) else {
    FileHandle.standardError.write("could not create the virtual display\n".data(using: .utf8)!)
    exit(1)
}
let settings = CGVirtualDisplaySettings()
settings.hiDPI = 0
settings.modes = [CGVirtualDisplayMode(width: width, height: height, refreshRate: fps)]
guard display.apply(settings) else {
    FileHandle.standardError.write("could not set the display to \(width)x\(height) at \(fps) Hz\n".data(using: .utf8)!)
    exit(1)
}

// macOS remembers the size it last used for a display with this identity and picks that again,
// even when this mode is the one asked for (seen 2026-11-18: 1920x1080 kept for a 2560x1440
// request). Switch to the exact size, for this login session.
func useRequestedMode(_ id: CGDirectDisplayID) -> Bool {
    let options = [kCGDisplayShowDuplicateLowResolutionModes: kCFBooleanTrue] as CFDictionary
    for _ in 0..<40 {
        let modes = (CGDisplayCopyAllDisplayModes(id, options) as? [CGDisplayMode]) ?? []
        if let mode = modes.first(where: {
            $0.width == Int(width) && $0.height == Int(height) && $0.pixelWidth == Int(width)
        }) {
            if let current = CGDisplayCopyDisplayMode(id), current.width == mode.width, current.height == mode.height {
                return true
            }
            var config: CGDisplayConfigRef?
            guard CGBeginDisplayConfiguration(&config) == .success else { return false }
            CGConfigureDisplayWithDisplayMode(config, id, mode, nil)
            return CGCompleteDisplayConfiguration(config, .forSession) == .success
        }
        usleep(50_000)
    }
    return false
}

if !useRequestedMode(display.displayID) {
    FileHandle.standardError.write("could not switch the display to \(width)x\(height)\n".data(using: .utf8)!)
}

print("ready \(display.displayID)")
fflush(stdout)

// The app holds our stdin open; when it closes (the app quit or crashed), so do we.
Thread.detachNewThread {
    while readLine() != nil {}
    exit(0)
}

withExtendedLifetime(display) {
    dispatchMain()
}
