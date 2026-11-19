// tray-icon.swift <out.png>: the menu-bar icon (macOS template image: black on transparent, the
// system tints it). 44x44 px = 22 pt at 2x.
import AppKit
let out = CommandLine.arguments[1]
let S = 44
let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: S, pixelsHigh: S, bitsPerSample: 8, samplesPerPixel: 4,
                           hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
NSGraphicsContext.saveGraphicsState()
NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
NSColor.black.setStroke(); NSColor.black.setFill()
// The extra screen behind, filled.
NSBezierPath(roundedRect: NSRect(x: 21, y: 17, width: 19, height: 14), xRadius: 2.5, yRadius: 2.5).fill()
// Clear a gap around the main screen so the two read apart.
NSGraphicsContext.current?.compositingOperation = .clear
NSBezierPath(roundedRect: NSRect(x: 2, y: 10, width: 27, height: 22), xRadius: 4.5, yRadius: 4.5).fill()
NSGraphicsContext.current?.compositingOperation = .sourceOver
// The main screen, outlined, with its stand.
let main = NSBezierPath(roundedRect: NSRect(x: 5, y: 13, width: 21, height: 16), xRadius: 3, yRadius: 3)
main.lineWidth = 3; main.stroke()
NSBezierPath(rect: NSRect(x: 14, y: 7, width: 3, height: 6)).fill()
NSBezierPath(roundedRect: NSRect(x: 9, y: 5, width: 13, height: 3), xRadius: 1.5, yRadius: 1.5).fill()
NSGraphicsContext.restoreGraphicsState()
try! rep.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: out))
