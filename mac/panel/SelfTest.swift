// `expand-panel --check-fixture FILE...`: a self-test that needs no window. Each non-empty
// line of each fixture must decode as a `PanelDto` through the same path the stdin reader uses;
// then the protocol and the model's rules are checked. Prints "OK" lines, or "FAIL" and exits 1.
// The pairing code is never printed.

import Foundation

@MainActor
enum SelfTest {
    private static var failures = 0

    static func run(fixtures: [String]) -> Int32 {
        if fixtures.isEmpty { fail("no fixture given") }
        for path in fixtures { checkFixture(path) }
        checkSnapshot()
        checkJunk()
        checkActions()
        checkModel()
        checkLines()
        if failures == 0 { say("OK all") }
        return failures == 0 ? 0 : 1
    }

    // MARK: Fixtures

    private static func checkFixture(_ path: String) {
        guard let data = FileManager.default.contents(atPath: path), let text = String(data: data, encoding: .utf8)
        else { return fail("\(path): unreadable") }
        let lines = text.split(whereSeparator: \.isNewline).filter { !$0.allSatisfy(\.isWhitespace) }
        if lines.isEmpty { return fail("\(path): empty") }
        for (number, line) in lines.enumerated() {
            guard let dto = decodeState(Data(line.utf8)) else { return fail("\(path):\(number + 1): does not decode") }
            let model = PanelModel()
            model.output = { _ in }
            model.apply(dto)
            say(
                "OK \(path):\(number + 1): version \(dto.version), connected \(dto.connected?.key ?? "none"), "
                    + "\(dto.others.count) other devices, pairing \(dto.pairing == nil ? "closed" : "open"), "
                    + "missing \(dto.missing.count)")
        }
    }

    // MARK: Protocol

    /// Task 3's snapshot of the state line, exactly as the app writes it.
    private static let snapshot = #"{"version":"1.1.0","connected":{"key":"pc-b","name":"PC pc-b","address":"192.168.1.3","state":"connected","ms":4,"uses":{"mouse":true,"clipboard":true,"display":true},"display":"2560 × 1440 · 60 fps"},"others":[{"key":"pc-a","name":"PC pc-a","address":"192.168.1.2","state":"blocked","ms":null,"uses":null,"display":null}],"pairing":{"code":"123456","seconds_left":112},"permissions":{"screen":true,"accessibility":false},"missing":["Sunshine"],"stay_awake":true,"start_at_login":true}"#

    private static func checkSnapshot() {
        let before = failures
        guard let dto = decodeState(Data(snapshot.utf8)) else { return fail("the snapshot line does not decode") }
        let pc = dto.connected
        expect(dto.version == "1.1.0", "version")
        expect(pc?.key == "pc-b" && pc?.name == "PC pc-b" && pc?.address == "192.168.1.3", "connected PC")
        expect(pc?.state == "connected" && pc?.ms == 4, "connected state and ms")
        expect(pc?.connectedDetail == "192.168.1.3", "the address alone, no ms")
        expect(pc?.roundTripHelp == "Round trip · 4 ms", "the round trip in the tooltip")
        expect(pc?.uses == Uses(mouse: true, clipboard: true, display: true), "uses")
        expect(pc?.display == "2560 × 1440 · 60 fps", "display text")
        expect(dto.others.count == 1 && dto.others[0].isBlocked, "blocked other PC")
        expect(dto.others[0].roundTripHelp == nil, "no tooltip without a round trip")
        var idle = pc
        idle?.uses = Uses(mouse: false, clipboard: false, display: false)
        expect(idle != nil && idle?.roundTripHelp == nil, "no tooltip while nothing is in use")
        expect(dto.others[0].ms == nil && dto.others[0].uses == nil && dto.others[0].display == nil, "nulls")
        expect(dto.pairing?.code.count == 6 && dto.pairing?.secondsLeft == 112, "pairing")
        expect(dto.permissions.screen == true && dto.permissions.accessibility == false, "permissions")
        expect(dto.missing == ["Sunshine"], "missing")
        expect(dto.stayAwake && dto.startAtLogin, "switches")

        let empty = #"{"version":"1.1.0","connected":null,"others":[],"pairing":null,"permissions":{"screen":null,"accessibility":null},"missing":[],"stay_awake":false,"start_at_login":false}"#
        let none = decodeState(Data(empty.utf8))
        expect(none != nil && none?.connected == nil && none?.pairing == nil, "the empty state")
        expect(none?.permissions.screen == nil && none?.missing == [], "unknown permissions, nothing missing")

        // Unknown fields and a state this panel doesn't know are taken as they come.
        let newer = #"{"version":"9","later":{"x":1},"connected":null,"others":[{"key":"k","name":"n","address":"a","state":"sleeping","ms":null,"uses":null,"display":null,"icon":"laptop"}],"pairing":null,"permissions":{"screen":true,"accessibility":true},"missing":[],"stay_awake":true,"start_at_login":false}"#
        let later = decodeState(Data(newer.utf8))
        expect(later?.others.first?.state == "sleeping", "unknown fields and states")
        if failures == before { say("OK state lines: snapshot, empty, newer") }
    }

    private static func checkJunk() {
        let junk = [
            "", "junk", "null", "[]", "{}", #"{"version":"1.1.0"}"#,
            #"{"action":"quit"}"#, String(snapshot.dropLast()),
            snapshot.replacingOccurrences(of: #""ms":4"#, with: #""ms":"4""#),
        ]
        let decoded = junk.filter { decodeState(Data($0.utf8)) != nil }
        expect(decoded.isEmpty, "junk lines are ignored (\(decoded.count) decoded)")
        if decoded.isEmpty { say("OK junk lines ignored: \(junk.count)") }
    }

    /// Every action, byte for byte as in the Task 3 protocol.
    private static func checkActions() {
        let cases: [(PanelAction, String)] = [
            (.connect(key: "pc-a"), #"{"action":"connect","key":"pc-a"}"#),
            (.disconnect, #"{"action":"disconnect"}"#),
            (.restart, #"{"action":"restart"}"#),
            (.block(key: "pc-a"), #"{"action":"block","key":"pc-a"}"#),
            (.allow(key: "pc-a"), #"{"action":"allow","key":"pc-a"}"#),
            (.forget(key: "pc-a"), #"{"action":"forget","key":"pc-a"}"#),
            (.addPc, #"{"action":"add_pc"}"#),
            (.cancelPairing, #"{"action":"cancel_pairing"}"#),
            (.panelClosed, #"{"action":"panel_closed"}"#),
            (.setStayAwake(on: true), #"{"action":"set_stay_awake","on":true}"#),
            (.setStartAtLogin(on: false), #"{"action":"set_start_at_login","on":false}"#),
            (.allowPermission(kind: .screen), #"{"action":"allow_permission","kind":"screen"}"#),
            (.allowPermission(kind: .accessibility), #"{"action":"allow_permission","kind":"accessibility"}"#),
            (.installMissing, #"{"action":"install_missing"}"#),
            (.quit, #"{"action":"quit"}"#),
        ]
        var bad = 0
        for (action, expected) in cases where action.line != Data((expected + "\n").utf8) {
            bad += 1
            fail("action encodes as \(String(decoding: action.line, as: UTF8.self)), expected \(expected)")
        }
        // A key with characters JSON escapes still round-trips.
        let odd = PanelAction.forget(key: "a\"b/c ×").line
        let back = (try? JSONSerialization.jsonObject(with: odd)) as? [String: String]
        expect(back == ["action": "forget", "key": "a\"b/c ×"], "an odd key round-trips")
        if bad == 0 { say("OK actions: \(cases.count) encode exactly") }
    }

    // MARK: The model's rules

    private static func checkModel() {
        let before = failures
        guard let full = decodeState(Data(snapshot.utf8)) else { return }
        var sent: [String] = []
        let model = PanelModel()
        model.output = { sent.append(String(decoding: $0, as: UTF8.self)) }
        let start = Date()
        model.apply(full, now: start)

        // The countdown runs from seconds_left, and the code goes when it reaches 0.
        expect(model.secondsLeft(at: start) == 112, "seconds left at the start")
        expect(model.secondsLeft(at: start.addingTimeInterval(40.2)) == 72, "seconds left later")
        expect(model.pairingCode(at: start.addingTimeInterval(111)) != nil, "the code before the end")
        expect(model.pairingCode(at: start.addingTimeInterval(113)) == nil, "no code after the end")
        expect(PairingView.clock(112) == "1:52" && PairingView.clock(5) == "0:05", "the clock")
        var long = full
        long.pairing?.secondsLeft = 121
        model.apply(long, now: start)
        expect(model.secondsLeft(at: start) == 120, "the countdown never shows more than 2:00")
        model.apply(full, now: start)
        expect(PairingView.grouped("123456") == "123 456", "the code in two groups")

        // Pairing closed by the app: the code is gone at once.
        var closed = full
        closed.pairing = nil
        model.apply(closed)
        expect(model.pairingCode(at: start) == nil, "no code once pairing is closed")

        // An action about a PC that has gone is dropped; one about a known PC is sent.
        sent.removeAll()
        model.send(.forget(key: "gone"))
        model.send(.connect(key: "gone"))
        expect(sent.isEmpty, "actions about a PC that has gone are dropped")
        model.send(.allow(key: "pc-a"))
        model.send(.disconnect)
        expect(sent == [#"{"action":"allow","key":"pc-a"}"# + "\n", #"{"action":"disconnect"}"# + "\n"], "actions sent")

        // The inline Forget question stays while its PC is unchanged, and goes when the PC's state
        // changes or it leaves the list.
        model.askForget("gone")
        expect(model.confirmingForget == nil, "no Forget question for a PC that has gone")
        model.askForget("pc-a")
        model.apply(closed)
        expect(model.confirmingForget == "pc-a", "the Forget question stays while nothing changes")
        var allowed = closed
        allowed.others[0].state = "ready"
        model.apply(allowed)
        expect(model.confirmingForget == nil, "the Forget question goes when its PC's state changes")
        model.askForget("pc-b")
        var swapped = allowed
        swapped.connected = nil
        swapped.others.append(PanelPc(key: "pc-b", name: "PC pc-b", address: "192.168.1.3", state: "ready"))
        model.apply(swapped)
        expect(model.confirmingForget == nil, "the Forget question goes when its PC disconnects")
        model.askForget("pc-a")
        var without = closed
        without.others = []
        model.apply(without)
        expect(model.confirmingForget == nil, "the Forget question goes with its PC")

        // A switch moves at once, and the next state from the app wins.
        model.send(.setStayAwake(on: false))
        expect(model.state?.stayAwake == false, "the switch moves at once")
        model.apply(without)
        expect(model.state?.stayAwake == true, "the app's state wins")

        // A Connect shows "Connecting" until the result, and a failed one doesn't stick.
        var ready = without
        ready.connected = nil
        ready.others = [PanelPc(key: "pc-a", name: "PC pc-a", address: "192.168.1.2", state: "ready")]
        model.apply(ready)
        model.send(.connect(key: "pc-a"))
        let first = model.connectAttempt
        model.apply(ready)  // the app's push after a failed attempt is the same state
        expect(model.connectingKey == "pc-a", "connecting survives an unchanged state")
        model.send(.connect(key: "pc-a"))
        model.connectTimedOut(first)
        expect(model.connectingKey == "pc-a", "an older attempt's timeout leaves a newer one")
        model.connectTimedOut(model.connectAttempt)
        expect(model.connectingKey == nil, "the timeout clears connecting")
        var other = ready
        other.connected = full.connected  // another PC (pc-b) is connected
        model.apply(other)
        model.send(.connect(key: "pc-a"))
        model.apply(other)
        expect(model.connectingKey == "pc-a", "another PC being connected leaves connecting")
        var mine = ready
        mine.connected = PanelPc(key: "pc-a", name: "PC pc-a", address: "192.168.1.2", state: "connected", ms: 3)
        mine.others = []
        model.apply(mine)
        expect(model.connectingKey == nil, "this PC connected clears connecting")
        model.apply(ready)
        model.send(.connect(key: "pc-a"))
        var moving = ready
        moving.others[0].state = "connecting"
        model.apply(moving)
        expect(model.connectingKey == nil, "a state other than ready clears connecting")
        model.apply(ready)
        model.send(.connect(key: "pc-a"))
        model.apply(full)
        expect(model.connectingKey == nil, "a blocked PC clears connecting")
        model.apply(ready)
        model.send(.connect(key: "pc-a"))
        model.panelClosed()
        model.panelOpened()
        expect(model.connectingKey == nil, "reopening clears connecting")

        // A Restart keeps its PC in the connected section through the drop, until it is back.
        model.apply(full)
        model.send(.restart)
        expect(model.restarting?.key == "pc-b", "restarting")
        model.apply(full)  // the app's push right after the action: not dropped yet
        expect(model.restarting?.key == "pc-b", "restarting survives the state before the drop")
        var dropped = full
        dropped.connected = nil
        dropped.others.append(PanelPc(key: "pc-b", name: "PC pc-b", address: "192.168.1.3", state: "ready"))
        model.apply(dropped)
        expect(model.restarting?.key == "pc-b" && model.shownConnected?.key == "pc-b", "shown connected while dropped")
        expect(model.shownOthers.map(\.key) == ["pc-a"], "not listed twice while dropped")
        model.apply(full)
        expect(model.restarting == nil && model.shownConnected?.key == "pc-b", "back: restarting ends")
        model.send(.restart)
        let firstRestart = model.restartAttempt
        model.apply(dropped)
        model.send(.restart)
        model.restartTimedOut(firstRestart)
        expect(model.restarting != nil, "an older restart's timeout leaves a newer one")
        model.restartTimedOut(model.restartAttempt)
        expect(model.restarting == nil && model.shownConnected == nil, "the timeout ends restarting")
        model.apply(full)
        model.send(.restart)
        var otherPc = full
        otherPc.connected = PanelPc(key: "pc-c", name: "PC pc-c", address: "192.168.1.4", state: "connected")
        model.apply(otherPc)
        expect(model.restarting == nil, "another PC connecting ends restarting")
        model.apply(full)
        model.send(.restart)
        model.send(.disconnect)
        expect(model.restarting == nil, "Disconnect ends restarting")
        model.send(.restart)
        var goneB = dropped
        goneB.others.removeAll { $0.key == "pc-b" }
        model.apply(goneB)
        expect(model.restarting == nil, "a PC that has gone ends restarting")

        // Closing the panel sends panel_closed once, and the code goes at once, before the app's
        // next state.
        let reopen = Date()
        model.apply(full, now: reopen)
        sent.removeAll()
        model.panelOpened()
        expect(model.pairingCode(at: reopen) != nil, "the code while the panel is open")
        model.panelClosed()
        model.panelClosed()
        expect(sent == [#"{"action":"panel_closed"}"# + "\n"], "panel_closed once per close")
        model.panelOpened()
        expect(model.pairingCode(at: reopen) == nil, "no code on a quick reopen")
        if failures == before { say("OK model: countdown, stale keys, forget, switches, connecting, restart, close") }
    }

    // MARK: Reading lines

    private static func checkLines() {
        let before = failures
        var splitter = LineSplitter()
        splitter.maxLength = 10
        var lines: [String] = []
        let feeds = ["ab", "c\n0123456789", "ABCDEF", "GH\nok\r\n", "1234567890\n", "tail", "\n"]
        for feed in feeds {
            lines += splitter.feed(Data(feed.utf8)).map { String(decoding: $0, as: UTF8.self) }
        }
        expect(lines == ["abc", "ok", "1234567890", "tail"], "lines split, CR dropped, a long line dropped: \(lines)")
        var real = LineSplitter()
        var big = Data(repeating: 0x20, count: (1 << 20) + 1)
        big.append(contentsOf: Array("\n\(snapshot)\n".utf8))
        let kept = real.feed(big)
        expect(kept.count == 1 && decodeState(kept[0]) != nil, "a line over 1 MB is dropped, the next one kept")
        if failures == before { say("OK lines: split across reads, CR, over-long lines dropped") }
    }

    // MARK: Output

    private static func expect(_ ok: Bool, _ what: String) {
        if !ok { fail(what) }
    }

    private static func fail(_ what: String) {
        failures += 1
        say("FAIL \(what)")
    }

    private static func say(_ line: String) {
        FileHandle.standardOutput.write(Data((line + "\n").utf8))
    }
}
