// app-icon.swift <out.png> [size]: draws the Sidescreen app icon. The app's icon files are made
// from it with `npx tauri icon` (see design/README.md).
import AppKit

func hex(_ v: UInt32, _ a: CGFloat = 1) -> NSColor {
    NSColor(srgbRed: CGFloat((v >> 16) & 255) / 255, green: CGFloat((v >> 8) & 255) / 255, blue: CGFloat(v & 255) / 255, alpha: a)
}
let args = CommandLine.arguments
let out = args[1]
let S: CGFloat = 1024
let shape = NSRect(x: 100, y: 100, width: 824, height: 824)
func squircle() -> NSBezierPath { NSBezierPath(roundedRect: shape, xRadius: 185, yRadius: 185) }
func rounded(_ r: NSRect, _ radius: CGFloat) -> NSBezierPath { NSBezierPath(roundedRect: r, xRadius: radius, yRadius: radius) }

let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: Int(S), pixelsHigh: Int(S), bitsPerSample: 8,
                           samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
NSGraphicsContext.saveGraphicsState()
NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)

// Base: a dark graphite squircle with a soft drop shadow and a faint rim.
let sh = NSShadow(); sh.shadowColor = NSColor.black.withAlphaComponent(0.35); sh.shadowBlurRadius = 26; sh.shadowOffset = NSSize(width: 0, height: -10)
NSGraphicsContext.saveGraphicsState(); sh.set(); NSColor.black.setFill(); squircle().fill(); NSGraphicsContext.restoreGraphicsState()
NSGraphicsContext.saveGraphicsState(); squircle().addClip()
NSGradient(starting: hex(0x26262A), ending: hex(0x111113))!.draw(in: shape, angle: -90)

// The extra screen, sliding out to the right from behind the main one.
let side = NSRect(x: 455, y: 475, width: 330, height: 225)
let sideGlow = NSShadow(); sideGlow.shadowColor = hex(0x3FD9B0, 0.35); sideGlow.shadowBlurRadius = 40
NSGraphicsContext.saveGraphicsState(); sideGlow.set(); hex(0x2FB894).setFill(); rounded(side, 26).fill(); NSGraphicsContext.restoreGraphicsState()
NSGradient(colors: [hex(0x6FE6C2), hex(0x34C3A0), hex(0x22947C)])!.draw(in: rounded(side, 26), angle: -60)

// The main screen in front: light graphite glass with a dark screen inset.
let main = NSRect(x: 230, y: 400, width: 400, height: 280)
let lift = NSShadow(); lift.shadowColor = NSColor.black.withAlphaComponent(0.55); lift.shadowBlurRadius = 30; lift.shadowOffset = NSSize(width: 0, height: -12)
NSGraphicsContext.saveGraphicsState(); lift.set(); hex(0xE8E8EC).setFill(); rounded(main, 30).fill(); NSGraphicsContext.restoreGraphicsState()
NSGradient(starting: hex(0xF4F4F7), ending: hex(0xC9C9D0))!.draw(in: rounded(main, 30), angle: -90)
let inset = main.insetBy(dx: 18, dy: 18)
NSGradient(starting: hex(0x2C2C31), ending: hex(0x18181B))!.draw(in: rounded(inset, 16), angle: -90)
// A stand under the main screen.
let neck = NSRect(x: 408, y: 338, width: 44, height: 62)
NSGradient(starting: hex(0xBDBDC5), ending: hex(0x9A9AA3))!.draw(in: rounded(neck, 6), angle: -90)
let foot = NSRect(x: 340, y: 322, width: 180, height: 26)
NSGradient(starting: hex(0xD6D6DC), ending: hex(0xA9A9B1))!.draw(in: rounded(foot, 13), angle: -90)
// A hint of the pointer crossing over: a small arrow at the seam.
let arrow = NSBezierPath()
arrow.move(to: NSPoint(x: 646, y: 590)); arrow.line(to: NSPoint(x: 646, y: 514)); arrow.line(to: NSPoint(x: 664, y: 532))
arrow.line(to: NSPoint(x: 678, y: 500)); arrow.line(to: NSPoint(x: 690, y: 506)); arrow.line(to: NSPoint(x: 676, y: 538))
arrow.line(to: NSPoint(x: 700, y: 540)); arrow.close()
let arrowShadow = NSShadow(); arrowShadow.shadowColor = NSColor.black.withAlphaComponent(0.4); arrowShadow.shadowBlurRadius = 8; arrowShadow.shadowOffset = NSSize(width: 0, height: -3)
NSGraphicsContext.saveGraphicsState(); arrowShadow.set(); NSColor.white.setFill(); arrow.fill(); NSGraphicsContext.restoreGraphicsState()
hex(0x111113).setStroke(); arrow.lineWidth = 5; arrow.stroke(); NSColor.white.setFill(); arrow.fill()
NSGraphicsContext.restoreGraphicsState()
let rim = squircle(); rim.lineWidth = 4; NSColor.white.withAlphaComponent(0.10).setStroke(); rim.stroke()

NSGraphicsContext.restoreGraphicsState()
try! rep.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: out))
