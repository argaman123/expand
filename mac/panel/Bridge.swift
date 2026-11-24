// The link to the Sidescreen app: JSON lines. A `PanelDto` arrives on stdin after start and on
// every change, and replaces the whole model. Actions go out on stdout, one line each. EOF on
// stdin means quit. Nothing here is ever logged: the state can hold the pairing code.

import Foundation
import Observation

// MARK: - Rust -> panel

struct PanelDto: Decodable, Equatable {
    var version: String
    var connected: PanelPc?
    var others: [PanelPc]
    var pairing: Pairing?
    var permissions: Permissions
    var missing: [String]
    var stayAwake: Bool
    var startAtLogin: Bool

    /// The PC with this key, connected or not.
    func pc(_ key: String) -> PanelPc? {
        connected?.key == key ? connected : others.first { $0.key == key }
    }
}

struct PanelPc: Decodable, Equatable, Identifiable {
    var key: String
    var name: String
    var address: String
    /// "ready", "blocked", "connected" or "connecting". Kept as text so a new state never breaks
    /// decoding.
    var state: String
    var ms: Int?
    var uses: Uses?
    var display: String?

    var id: String { key }
    var isBlocked: Bool { state == "blocked" }
    var isConnecting: Bool { state == "connecting" }
}

struct Uses: Decodable, Equatable {
    var mouse: Bool
    var clipboard: Bool
    var display: Bool
}

struct Pairing: Decodable, Equatable {
    var code: String
    var secondsLeft: Int
}

struct Permissions: Decodable, Equatable {
    var screen: Bool?
    var accessibility: Bool?
}

/// Decodes one line from the app. A line that isn't a valid `PanelDto` gives nil and is ignored.
func decodeState(_ line: Data) -> PanelDto? {
    let decoder = JSONDecoder()
    decoder.keyDecodingStrategy = .convertFromSnakeCase
    return try? decoder.decode(PanelDto.self, from: line)
}

// MARK: - panel -> Rust

enum PermissionKind: String { case screen, accessibility }

enum PanelAction: Equatable {
    case connect(key: String)
    case disconnect
    case restart
    case block(key: String)
    case allow(key: String)
    case forget(key: String)
    case addPc
    case cancelPairing
    case panelClosed
    case setStayAwake(on: Bool)
    case setStartAtLogin(on: Bool)
    case allowPermission(kind: PermissionKind)
    case installMissing
    case quit

    /// The PC this action is about, if any.
    var key: String? {
        switch self {
        case .connect(let key), .block(let key), .allow(let key), .forget(let key): return key
        default: return nil
        }
    }

    /// The JSON object, as `{"action": ..., ...}`.
    var fields: [String: Any] {
        switch self {
        case .connect(let key): return ["action": "connect", "key": key]
        case .disconnect: return ["action": "disconnect"]
        case .restart: return ["action": "restart"]
        case .block(let key): return ["action": "block", "key": key]
        case .allow(let key): return ["action": "allow", "key": key]
        case .forget(let key): return ["action": "forget", "key": key]
        case .addPc: return ["action": "add_pc"]
        case .cancelPairing: return ["action": "cancel_pairing"]
        case .panelClosed: return ["action": "panel_closed"]
        case .setStayAwake(let on): return ["action": "set_stay_awake", "on": on]
        case .setStartAtLogin(let on): return ["action": "set_start_at_login", "on": on]
        case .allowPermission(let kind): return ["action": "allow_permission", "kind": kind.rawValue]
        case .installMissing: return ["action": "install_missing"]
        case .quit: return ["action": "quit"]
        }
    }

    /// One line, with its newline.
    var line: Data {
        var data = (try? JSONSerialization.data(withJSONObject: fields, options: [.sortedKeys, .withoutEscapingSlashes])) ?? Data()
        data.append(0x0A)
        return data
    }
}

// MARK: - The model

@MainActor
@Observable
final class PanelModel {
    /// The latest state from the app; nil until the first line arrives.
    private(set) var state: PanelDto?
    /// When pairing ends, counted locally from `seconds_left` and re-synced on every update.
    private(set) var pairingEnds: Date?
    /// The PC whose row asks "Forget / Cancel". The question goes when that PC's state changes or
    /// it leaves the list.
    private(set) var confirmingForget: String?
    private var confirmingForgetState: String?
    /// A Connect pressed in this panel, shown until that PC connects, leaves "ready" or goes, the
    /// panel reopens, or `connectTimeout` passes. The app pushes the same state after a failed
    /// attempt, so the timeout is what clears a failure.
    private(set) var connectingKey: String?
    /// Counts Connect presses, so an older attempt's timeout leaves a newer one alone.
    private(set) var connectAttempt = 0
    static let connectTimeout: TimeInterval = 10

    /// The connected PC while a Restart pressed here is under way. It stays in the connected
    /// section (so the panel doesn't jump) until it is connected again after dropping, another PC
    /// connects, it goes or is blocked, Disconnect is pressed, or `restartTimeout` passes.
    private(set) var restarting: PanelPc?
    private var restartSawDrop = false
    /// Counts Restart presses, so an older attempt's timeout leaves a newer one alone.
    private(set) var restartAttempt = 0
    static let restartTimeout: TimeInterval = 10

    /// What the connected section shows: the connected PC, or the one restarting.
    var shownConnected: PanelPc? {
        state?.connected ?? restarting
    }

    /// The other PCs, without the one restarting.
    var shownOthers: [PanelPc] {
        guard let state else { return [] }
        guard let pc = restarting, state.connected == nil else { return state.others }
        return state.others.filter { $0.key != pc.key }
    }

    /// Pairing mode lasts at most 2 minutes; the countdown never shows more.
    static let pairingWindow = 120

    /// Where actions go. Standard output by default; the self-test swaps it.
    var output: (Data) -> Void = Bridge.write

    func apply(_ dto: PanelDto, now: Date = Date()) {
        if let pairing = dto.pairing {
            let seconds = min(max(0, pairing.secondsLeft), Self.pairingWindow)
            pairingEnds = now.addingTimeInterval(TimeInterval(seconds))
        } else {
            pairingEnds = nil
        }
        if let key = confirmingForget, dto.pc(key)?.state != confirmingForgetState {
            confirmingForget = nil
        }
        if let key = connectingKey {
            let pc = dto.pc(key)
            if pc == nil || dto.connected?.key == key || pc?.state != "ready" { connectingKey = nil }
        }
        if let key = restarting?.key {
            let pc = dto.pc(key)
            if pc == nil || pc?.isBlocked == true || (dto.connected != nil && dto.connected?.key != key) {
                restarting = nil  // gone, blocked, or another PC took over
            } else if dto.connected?.key == key {
                if restartSawDrop { restarting = nil } else { restarting = dto.connected }
            } else {
                restartSawDrop = true
            }
        }
        state = dto
    }

    func knows(_ key: String) -> Bool {
        state?.pc(key) != nil
    }

    func askForget(_ key: String) {
        guard let pc = state?.pc(key) else { return }
        confirmingForget = key
        confirmingForgetState = pc.state
    }

    func cancelForget() {
        confirmingForget = nil
    }

    /// Sends an action. One about a PC that is no longer in the state is dropped.
    func send(_ action: PanelAction) {
        if let key = action.key, !knows(key) { return }
        switch action {
        case .restart:
            if let pc = shownConnected {
                restarting = pc
                restartSawDrop = state?.connected == nil
                restartAttempt += 1
                let attempt = restartAttempt
                DispatchQueue.main.asyncAfter(deadline: .now() + Self.restartTimeout) { [weak self] in
                    MainActor.assumeIsolated { self?.restartTimedOut(attempt) }
                }
            }
        case .disconnect:
            restarting = nil
        case .connect(let key):
            connectingKey = key
            connectAttempt += 1
            let attempt = connectAttempt
            DispatchQueue.main.asyncAfter(deadline: .now() + Self.connectTimeout) { [weak self] in
                MainActor.assumeIsolated { self?.connectTimedOut(attempt) }
            }
        case .setStayAwake(let on):
            state?.stayAwake = on  // the app's next state corrects it if the change failed
        case .setStartAtLogin(let on):
            state?.startAtLogin = on
        case .forget(let key):
            if confirmingForget == key { confirmingForget = nil }
        default:
            break
        }
        output(action.line)
    }

    func connectTimedOut(_ attempt: Int) {
        if attempt == connectAttempt { connectingKey = nil }
    }

    func restartTimedOut(_ attempt: Int) {
        if attempt == restartAttempt { restarting = nil }
    }

    /// The panel is on screen; its gradients move only then.
    private(set) var isOpen = false

    func panelOpened() {
        guard !isOpen else { return }
        isOpen = true
        confirmingForget = nil
        connectingKey = nil
    }

    /// The panel was closed: ends pairing (the app does that on `panel_closed`).
    func panelClosed() {
        guard isOpen else { return }
        isOpen = false
        PanelLog.write("panel closed")
        confirmingForget = nil
        pairingEnds = nil  // the app ends pairing now; a quick reopen mustn't show the code
        send(.panelClosed)
    }

    /// The pairing code while pairing is open and its time hasn't run out here.
    func pairingCode(at now: Date) -> String? {
        guard let code = state?.pairing?.code, let ends = pairingEnds, ends > now else { return nil }
        return code
    }

    func secondsLeft(at now: Date) -> Int {
        guard let ends = pairingEnds else { return 0 }
        return max(0, Int(ends.timeIntervalSince(now).rounded(.up)))
    }
}

// MARK: - stdin and stdout

enum Bridge {
    private static let lock = NSLock()

    /// Writes one action line to stdout. A closed pipe is ignored: stdin's EOF ends the panel.
    static func write(_ line: Data) {
        lock.lock()
        defer { lock.unlock() }
        try? FileHandle.standardOutput.write(contentsOf: line)
    }

    /// Reads state lines on a background thread. EOF quits the panel, on the main thread.
    static func startReading(into model: PanelModel) {
        signal(SIGPIPE, SIG_IGN)
        let thread = Thread {
            var splitter = LineSplitter()
            var chunk = [UInt8](repeating: 0, count: 64 * 1024)
            var lastSummary = ""
            while true {
                let count = read(STDIN_FILENO, &chunk, chunk.count)
                if count < 0 && errno == EINTR { continue }
                if count <= 0 { break }
                let dropped = splitter.dropped
                for line in splitter.feed(Data(chunk[0..<count])) {
                    guard let dto = decodeState(line) else {
                        PanelLog.write("state line ignored: \(line.count) bytes do not decode")
                        continue
                    }
                    let summary = PanelLog.summary(dto)
                    if summary != lastSummary {
                        lastSummary = summary
                        PanelLog.write(summary)
                    }
                    DispatchQueue.main.async {
                        MainActor.assumeIsolated { model.apply(dto) }
                    }
                }
                if splitter.dropped > dropped { PanelLog.write("state line over 1 MB dropped") }
            }
            PanelLog.write("stdin closed: quitting")
            DispatchQueue.main.async { exit(0) }
            // Should the main thread be stuck, end anyway: never outlive the app.
            Thread.sleep(forTimeInterval: 3)
            exit(0)
        }
        thread.name = "state reader"
        thread.start()
    }
}

/// Splits bytes into lines at LF, dropping a trailing CR. A line longer than `maxLength` is
/// dropped whole, up to its newline.
struct LineSplitter {
    var maxLength = 1 << 20
    private var buffer = Data()
    private var dropping = false
    /// How many over-long lines were dropped.
    private(set) var dropped = 0

    mutating func feed(_ chunk: Data) -> [Data] {
        var lines: [Data] = []
        var start = chunk.startIndex
        while let newline = chunk[start...].firstIndex(of: 0x0A) {
            let piece = chunk[start..<newline]
            if !dropping && buffer.count + piece.count <= maxLength {
                var line = buffer
                line.append(contentsOf: piece)
                if line.last == 0x0D { line.removeLast() }
                lines.append(line)
            }
            buffer.removeAll(keepingCapacity: true)
            dropping = false
            start = chunk.index(after: newline)
        }
        let rest = chunk[start...]
        if !dropping {
            if buffer.count + rest.count > maxLength {
                dropping = true
                dropped += 1
                buffer.removeAll()
            } else {
                buffer.append(contentsOf: rest)
            }
        }
        return lines
    }
}
