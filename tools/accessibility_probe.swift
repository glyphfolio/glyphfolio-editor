#!/usr/bin/env swift
// Opt-in probe for the exact standalone synthetic demo process only.
// No existing GlyphFolio application, user documents, or clipboard are touched.
// VoiceOver is optional and restored to its observed pre-test state.
import AppKit
import ApplicationServices
import Carbon
import CryptoKit
import Foundation
import Darwin

enum ProbeFailure: Error, CustomStringConvertible {
    case invalidArguments
    case failed(String)
    var description: String {
        switch self {
        case .invalidArguments: return "Expected --pid DEMO_PID --output /absolute/path/platform-NAME.json [--with-voiceover]"
        case .failed(let message): return message
        }
    }
}

func require(_ condition: Bool, _ message: String) throws {
    if !condition { throw ProbeFailure.failed(message) }
}

func digest(_ value: String) -> String {
    SHA256.hash(data: Data(value.utf8)).map { String(format: "%02x", $0) }.joined()
}

func spin(_ seconds: Double) {
    let until = Date().addingTimeInterval(seconds)
    while Date() < until {
        RunLoop.current.run(until: min(until, Date().addingTimeInterval(0.025)))
    }
}

func wait(_ seconds: Double = 3, _ predicate: () -> Bool) -> Bool {
    let until = Date().addingTimeInterval(seconds)
    repeat {
        if predicate() { return true }
        spin(0.04)
    } while Date() < until
    return predicate()
}

func ax(_ element: AXUIElement, _ attribute: String) -> CFTypeRef? {
    AXUIElementSetMessagingTimeout(element, 0.5)
    var value: CFTypeRef?
    guard AXUIElementCopyAttributeValue(element, attribute as CFString, &value) == .success else { return nil }
    return value
}

func text(_ element: AXUIElement) -> String? {
    ax(element, kAXValueAttribute) as? String
}

func set(_ element: AXUIElement, _ attribute: String, _ value: CFTypeRef) throws {
    AXUIElementSetMessagingTimeout(element, 0.5)
    let result = AXUIElementSetAttributeValue(element, attribute as CFString, value)
    try require(result == .success, "AX setter rejected \(attribute), code=\(result.rawValue)")
}

func ownedDescendants(_ root: AXUIElement, pid: pid_t, limit: Int = 3000) -> [AXUIElement] {
    var queue = [root]
    var result: [AXUIElement] = []
    var visited: [AXUIElement] = []
    let deadline = Date().addingTimeInterval(5)
    while let element = queue.popLast(), result.count < limit, Date() < deadline {
        AXUIElementSetMessagingTimeout(element, 0.5)
        var owner: pid_t = 0
        guard AXUIElementGetPid(element, &owner) == .success, owner == pid,
              !visited.contains(where: { CFEqual($0, element) }) else { continue }
        visited.append(element)
        // Standard application menu bars can contain system recent-item menus.
        // The probe needs only this process's own window content.
        if (ax(element, kAXRoleAttribute) as? String) == kAXMenuBarRole { continue }
        result.append(element)
        if let children = ax(element, kAXChildrenAttribute) as? [AXUIElement] {
            queue.append(contentsOf: children.reversed())
        }
    }
    return result
}

func selectedRange(_ element: AXUIElement) -> CFRange? {
    guard let raw = ax(element, kAXSelectedTextRangeAttribute),
          CFGetTypeID(raw) == AXValueGetTypeID() else { return nil }
    let value = unsafeBitCast(raw, to: AXValue.self)
    var range = CFRange()
    return AXValueGetValue(value, .cfRange, &range) ? range : nil
}

func select(_ element: AXUIElement, _ range: NSRange) throws {
    var converted = CFRange(location: range.location, length: range.length)
    guard let value = AXValueCreate(.cfRange, &converted) else {
        throw ProbeFailure.failed("Cannot create native UTF-16 selection range")
    }
    try set(element, kAXSelectedTextRangeAttribute, value)
    try require(wait {
        selectedRange(element).map { $0.location == range.location && $0.length == range.length } ?? false
    }, "Native selected range did not match the requested UTF-16 range")
}

func frame(_ element: AXUIElement) -> CGRect? {
    guard let rawPosition = ax(element, kAXPositionAttribute),
          let rawSize = ax(element, kAXSizeAttribute),
          CFGetTypeID(rawPosition) == AXValueGetTypeID(), CFGetTypeID(rawSize) == AXValueGetTypeID() else { return nil }
    var point = CGPoint.zero
    var size = CGSize.zero
    let p = unsafeBitCast(rawPosition, to: AXValue.self)
    let s = unsafeBitCast(rawSize, to: AXValue.self)
    guard AXValueGetValue(p, .cgPoint, &point), AXValueGetValue(s, .cgSize, &size) else { return nil }
    return CGRect(origin: point, size: size)
}

func currentInputID() -> String? {
    guard let source = TISCopyCurrentKeyboardInputSource()?.takeRetainedValue(),
          let pointer = TISGetInputSourceProperty(source, kTISPropertyInputSourceID) else { return nil }
    return Unmanaged<CFString>.fromOpaque(pointer).takeUnretainedValue() as String
}

func frontmost(_ pid: pid_t) throws {
    let actual = NSWorkspace.shared.frontmostApplication?.processIdentifier ?? -1
    try require(actual == pid,
                "Refusing key events because the standalone demo is not frontmost (expectedPID=\(pid), actualPID=\(actual))")
}

func modifierEvents(_ flags: CGEventFlags) throws -> ([CGEvent], [CGEvent]) {
    let modifiers: [(CGEventFlags, CGKeyCode)] = [(.maskControl, 59), (.maskAlternate, 58), (.maskShift, 56), (.maskCommand, 55), (.maskSecondaryFn, 63)]
    var accumulated: CGEventFlags = []
    var down: [CGEvent] = []
    var up: [CGEvent] = []
    for (mask, code) in modifiers where flags.contains(mask) {
        accumulated.insert(mask)
        guard let event = CGEvent(keyboardEventSource: nil, virtualKey: code, keyDown: true) else {
            throw ProbeFailure.failed("Cannot construct native modifier transition")
        }
        event.type = .flagsChanged
        event.flags = accumulated
        down.append(event)
    }
    for (mask, code) in modifiers.reversed() where flags.contains(mask) {
        accumulated.remove(mask)
        guard let event = CGEvent(keyboardEventSource: nil, virtualKey: code, keyDown: false) else {
            throw ProbeFailure.failed("Cannot construct native modifier restoration")
        }
        event.type = .flagsChanged
        event.flags = accumulated
        up.append(event)
    }
    return (down, up)
}

func post(_ event: CGEvent, to pid: pid_t, globalSystem: Bool) {
    if globalSystem { event.post(tap: .cghidEventTap) }
    else { event.postToPid(pid) }
}

func key(_ pid: pid_t, _ code: CGKeyCode, flags: CGEventFlags = [], globalVoiceOver: Bool = false) throws {
    try frontmost(pid)
    guard let down = CGEvent(keyboardEventSource: nil, virtualKey: code, keyDown: true),
          let up = CGEvent(keyboardEventSource: nil, virtualKey: code, keyDown: false) else {
        throw ProbeFailure.failed("Cannot construct native keyboard event")
    }
    down.flags = flags
    up.flags = flags
    if code == 6 {
        let character: [UniChar] = [122]
        character.withUnsafeBufferPointer { value in
            down.keyboardSetUnicodeString(stringLength: 1, unicodeString: value.baseAddress)
            up.keyboardSetUnicodeString(stringLength: 1, unicodeString: value.baseAddress)
        }
    }
    let (modifierDown, modifierUp) = try modifierEvents(flags)
    if globalVoiceOver {
        try require(try freshVoiceOverEnabled(), "Refusing a global VoiceOver shortcut without independently confirmed VoiceOver state")
    }
    modifierDown.forEach { post($0, to: pid, globalSystem: globalVoiceOver) }
    post(down, to: pid, globalSystem: globalVoiceOver)
    post(up, to: pid, globalSystem: globalVoiceOver)
    modifierUp.forEach { post($0, to: pid, globalSystem: globalVoiceOver) }
    spin(0.12)
}

func typeText(_ pid: pid_t, _ value: String) throws {
    try frontmost(pid)
    let utf16 = Array(value.utf16)
    guard let down = CGEvent(keyboardEventSource: nil, virtualKey: 0, keyDown: true),
          let up = CGEvent(keyboardEventSource: nil, virtualKey: 0, keyDown: false) else {
        throw ProbeFailure.failed("Cannot construct native Unicode keyboard event")
    }
    utf16.withUnsafeBufferPointer { buffer in
        down.keyboardSetUnicodeString(stringLength: buffer.count, unicodeString: buffer.baseAddress)
        up.keyboardSetUnicodeString(stringLength: buffer.count, unicodeString: buffer.baseAddress)
    }
    down.flags = []
    up.flags = []
    down.postToPid(pid)
    up.postToPid(pid)
    spin(0.2)
}

func toggleVoiceOver(_ pid: pid_t) throws {
    // This fixed Command-F5 sequence targets the approved system VoiceOver
    // service, not application text. Ordinary text/undo always use postToPid.
    // Attempt to keep our disposable demo foreground before/after the toggle.
    NSRunningApplication(processIdentifier: pid)?.activate(options: [.activateAllWindows])
    spin(0.1)
    guard let down = CGEvent(keyboardEventSource: nil, virtualKey: 96, keyDown: true),
          let up = CGEvent(keyboardEventSource: nil, virtualKey: 96, keyDown: false) else {
        throw ProbeFailure.failed("Cannot construct the normal Command-F5 VoiceOver toggle")
    }
    let flags: CGEventFlags = .maskCommand
    down.flags = flags
    up.flags = flags
    let (modifierDown, modifierUp) = try modifierEvents(flags)
    modifierDown.forEach { $0.post(tap: .cghidEventTap) }
    down.post(tap: .cghidEventTap)
    up.post(tap: .cghidEventTap)
    modifierUp.forEach { $0.post(tap: .cghidEventTap) }
    spin(0.2)
}

func boundedProcess(_ executable: String, _ arguments: [String], seconds: Double = 8) throws -> (Int32, String, String) {
    let process = Process()
    let output = Pipe()
    let failure = Pipe()
    process.executableURL = URL(fileURLWithPath: executable)
    process.arguments = arguments
    process.standardOutput = output
    process.standardError = failure
    try process.run()
    let completed = wait(seconds) { !process.isRunning }
    if !completed {
        process.terminate()
        if !wait(0.5, { !process.isRunning }) {
            // This is only our own just-created probe subprocess. Bound the
            // AppleEvent boundary even if a normal permission dialog is open.
            Darwin.kill(process.processIdentifier, SIGKILL)
            _ = wait(0.5) { !process.isRunning }
        }
        throw ProbeFailure.failed("Native helper subprocess timed out: \(URL(fileURLWithPath: executable).lastPathComponent)")
    }
    let stdout = String(data: output.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8) ?? ""
    let stderr = String(data: failure.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8) ?? ""
    return (process.terminationStatus, stdout, stderr)
}

func freshVoiceOverEnabled() throws -> Bool {
    // NSWorkspace can retain stale/delayed state during setup. Every start and
    // restoration gate also checks an independent process, rather than relying
    // on this long-lived probe's cached state or the presence of an app bundle.
    let (status, value, _) = try boundedProcess("/usr/bin/swift", [
        "-e", "import AppKit; print(NSWorkspace.shared.isVoiceOverEnabled ? \"1\" : \"0\")"
    ])
    let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
    try require(status == 0 && ["0", "1"].contains(trimmed), "Independent VoiceOver status was unavailable")
    return trimmed == "1"
}

func appleScript(_ source: String) throws -> String {
    let (status, value, failure) = try boundedProcess("/usr/bin/osascript", ["-e", source], seconds: 6)
    if status != 0 {
        // Keep only the numerical AppleEvent code. Never print arbitrary
        // speech text, application content, or the whole osascript diagnostic.
        let expression = try NSRegularExpression(pattern: #"\((-?[0-9]+)\)"#)
        let range = NSRange(failure.startIndex..., in: failure)
        let matches = expression.matches(in: failure, range: range)
        let code = matches.last.flatMap { Range($0.range(at: 1), in: failure) }.map { String(failure[$0]) } ?? "unavailable"
        throw ProbeFailure.failed("VoiceOver AppleScript unavailable, code=\(code); no scripting preference was changed")
    }
    return value.trimmingCharacters(in: .newlines)
}

func scriptBounds(_ value: String) throws -> CGRect {
    let components = value.split(separator: ",").compactMap { Double($0.trimmingCharacters(in: .whitespacesAndNewlines)) }
    try require(components.count == 4, "VoiceOver cursor bounds are unavailable")
    return CGRect(x: components[0], y: components[1], width: components[2] - components[0], height: components[3] - components[1])
}

final class Probe {
    let pid: pid_t
    let output: URL
    let withVoiceOver: Bool
    var checks: [[String: Any]] = []
    var app: NSRunningApplication?
    var area: AXUIElement?
    var priorText: String?
    var priorRange: CFRange?
    var voiceOverBefore = false
    var voiceOverStarted = false
    var textRestored = false
    var voiceOverRestored = false
    var restorationStarted = false
    var interruptedExit: Int32?
    var quickstartBefore = Set<pid_t>()
    var testQuickstart = Set<pid_t>()
    var welcomeAction: [String: Any]?
    var voiceOverAfterFresh: Bool?

    init(pid: pid_t, output: URL, withVoiceOver: Bool) {
        self.pid = pid
        self.output = output
        self.withVoiceOver = withVoiceOver
        self.voiceOverBefore = NSWorkspace.shared.isVoiceOverEnabled
        self.quickstartBefore = Set(NSRunningApplication.runningApplications(withBundleIdentifier: "com.apple.VoiceOverQuickstart").map(\.processIdentifier))
    }

    func record(_ name: String, _ status: String, _ detail: [String: Any] = [:]) {
        checks.append(["check": name, "status": status, "detail": detail])
        print("NATIVE_ACCESSIBILITY \(name)=\(status)")
    }

    func focus() throws {
        guard let app, let area else { throw ProbeFailure.failed("Demo focus target is unavailable") }
        app.activate(options: [.activateAllWindows])
        _ = AXUIElementSetAttributeValue(AXUIElementCreateApplication(pid), kAXFrontmostAttribute as CFString, kCFBooleanTrue)
        try require(wait { NSWorkspace.shared.frontmostApplication?.processIdentifier == self.pid }, "Demo did not become frontmost")
        try set(area, kAXFocusedAttribute, kCFBooleanTrue)
        spin(0.2)
        try frontmost(pid)
    }

    func setText(_ source: String) throws {
        guard let area else { throw ProbeFailure.failed("No standalone text area") }
        try set(area, kAXValueAttribute, source as CFString)
        try require(wait { text(area) == source }, "AXValue update was not acknowledged by the editor")
    }

    func run() throws {
        try require(AXIsProcessTrusted(), "Accessibility permission is not granted")
        try require(CGPreflightPostEventAccess(), "Native event posting is not authorized")
        guard let target = NSRunningApplication(processIdentifier: pid),
              target.executableURL?.lastPathComponent == "glyphfolio-editor-demo" else {
            throw ProbeFailure.failed("Refusing a process other than the standalone glyphfolio-editor-demo")
        }
        app = target
        voiceOverBefore = try freshVoiceOverEnabled()
        let root = AXUIElementCreateApplication(pid)
        try require(wait(5) {
            self.area = ownedDescendants(root, pid: self.pid).first { (ax($0, kAXRoleAttribute) as? String) == kAXTextAreaRole }
            return self.area != nil
        }, "Standalone AXTextArea was not exposed")
        guard let area, let original = text(area) else { throw ProbeFailure.failed("Own-demo source value is unavailable") }
        try require(original.isEmpty || (original.contains("glyphfolio-editor") && original.contains("feasibility")),
                    "Refusing to replace source other than the fresh synthetic demo fixture")
        priorText = original
        priorRange = selectedRange(area)
        record("native_text_area", "PASS", ["originalBytes": original.utf8.count, "originalSHA256": digest(original)])
        try require(currentInputID() == "com.apple.keylayout.US", "Run this probe after the IME driver has restored the US source; no source was changed")
        try focus()

        let fixture = "# native AX fixture\nalpha e\u{301} 日本 😀 beta\n" + (0..<70).map { "offscreen line \($0)\n" }.joined() + "final sentinel\n"
        try setText(fixture)
        record("native_set_value", "PASS", ["fixtureBytes": fixture.utf8.count, "fixtureSHA256": digest(fixture)])
        let selection = (fixture as NSString).range(of: "beta")
        try select(area, selection)
        try require((ax(area, kAXSelectedTextAttribute) as? String) == "beta", "Native selected text disagrees after UTF-16 selection")
        record("native_utf16_selection", "PASS", ["location": selection.location, "length": selection.length])
        try typeText(pid, "native")
        let replaced = (fixture as NSString).replacingCharacters(in: selection, with: "native")
        try require(wait { text(area) == replaced }, "Native keyboard replacement did not reach the selected source range")
        record("native_replace_selection", "PASS")
        try key(pid, 6, flags: .maskCommand)
        if wait(3, { text(area) == fixture }) {
            record("native_undo", "PASS")
        } else {
            let observed = text(area) ?? ""
            let rawZ = (replaced as NSString).replacingCharacters(in: NSRange(location: selection.location + 6, length: 0), with: "z")
            let classification = observed == replaced ? "shortcut_not_acted"
                : (observed == rawZ ? "raw_z_inserted_modifier_missing"
                : (observed == original ? "history_overshot_fixture" : "other_known_fixture_state"))
            record("native_undo", "FAIL", ["observedState": classification, "observedSHA256": digest(observed)])
            // Keep the failed transport evidence, but let the independent
            // screen-reader setup run against a fresh known fixture.
            try setText(fixture)
        }

        try offscreen(root: root, area: area)
        if withVoiceOver {
            do { try probeVoiceOver(area) }
            catch { record("voiceover_interaction", "PARTIAL", ["reason": String(describing: error)]) }
        } else {
            record("voiceover_interaction", "NOT_RUN", ["reason": "--with-voiceover was not requested"])
        }
    }

    func offscreen(root: AXUIElement, area: AXUIElement) throws {
        // TextRun nodes are intentionally consumed by the native text-range
        // adapter. Paragraph groups remain real, scrollable native targets.
        let paragraphs = ownedDescendants(area, pid: pid).filter { (ax($0, kAXRoleAttribute) as? String) == kAXGroupRole }
        guard let first = paragraphs.first, let last = paragraphs.last,
              let window = (ax(root, kAXWindowsAttribute) as? [AXUIElement])?.first,
              let windowFrame = frame(window) else {
            record("native_offscreen_navigation", "PARTIAL", ["reason": "Native paragraph/window bounds are unavailable"])
            return
        }
        let initialAction = AXUIElementPerformAction(first, "AXScrollToVisible" as CFString)
        if initialAction != .success {
            record("native_offscreen_navigation", "FAIL", ["actionCode": initialAction.rawValue])
            return
        }
        spin(0.2)
        guard let before = frame(last) else {
            record("native_offscreen_navigation", "PARTIAL", ["reason": "Offscreen paragraph bounds are unavailable"])
            return
        }
        let wasOffscreen = !windowFrame.intersects(before)
        let result = AXUIElementPerformAction(last, "AXScrollToVisible" as CFString)
        let moved = wait { frame(last).map { windowFrame.intersects($0) } ?? false }
        record("native_offscreen_navigation", result == .success && wasOffscreen && moved ? "PASS" : "FAIL", [
            "actionCode": result.rawValue, "wasOffscreen": wasOffscreen, "becameVisible": moved,
            "beforeY": before.minY, "afterY": frame(last)?.minY ?? -1,
        ])
    }

    func configureVoiceOver(_ enable: Bool) throws {
        let directory = URL(fileURLWithPath: #filePath).standardizedFileURL.deletingLastPathComponent()
        let cache = directory.deletingLastPathComponent().appendingPathComponent("target/swift-module-cache").path
        let options = enable ? ["--enable", "--with-script-control"] : ["--disable"]
        let (status, _, _) = try boundedProcess("/usr/bin/swift",
            ["-module-cache-path", cache, directory.appendingPathComponent("voiceover_setup.swift").path] + options,
            seconds: 35)
        try require(status == 0, "Normal VoiceOver setup/restoration failed, status=\(status); inspect its owned setup journal")
    }

    func probeVoiceOver(_ area: AXUIElement) throws {
        let fixture = "alpha beta gamma\nsecond sentence\n" + String(repeating: "source text\n", count: 70) + "final sentinel"
        try setText(fixture)
        try select(area, NSRange(location: 0, length: 0))
        try focus()
        if let first = ownedDescendants(area, pid: pid).first(where: { (ax($0, kAXRoleAttribute) as? String) == kAXGroupRole }) {
            AXUIElementSetMessagingTimeout(first, 0.5)
            try require(AXUIElementPerformAction(first, "AXScrollToVisible" as CFString) == .success,
                        "Cannot reset the synthetic text viewport before VoiceOver interaction")
            spin(0.3)
        }
        if !voiceOverBefore {
            // Use published normal UI controls with a restoration journal.
            // Function-key shortcuts may be disabled or intercepted by welcome.
            voiceOverStarted = true
            try configureVoiceOver(true)
            let independentlyEnabled = try freshVoiceOverEnabled()
            try require(independentlyEnabled, "VoiceOver was not independently enabled after the normal setup")
        }
        record("voiceover_enabled_flag", "PASS", ["wasEnabled": voiceOverBefore, "independentProcessVerified": true, "screenReaderInteractionQualified": false])
        try focus()
        // Setup temporarily enables only the journaled General scripting option;
        // restoration returns it to the exact prior state.
        _ = try scriptBounds(appleScript("tell application \"VoiceOver\" to get bounds of keyboard cursor"))
        try require(try freshVoiceOverEnabled(), "VoiceOver became disabled before screen-reader interaction")
        record("voiceover_script_transport", "PASS")
        let vo: CGEventFlags = [.maskControl, .maskAlternate]
        // Apple's documented VO-Shift-F4 moves the VoiceOver cursor to the
        // keyboard focus. VO-Shift-Down interacts with the focused text area.
        try key(pid, 118, flags: [vo, .maskShift, .maskSecondaryFn], globalVoiceOver: true)
        try key(pid, 125, flags: [vo, .maskShift], globalVoiceOver: true)
        try key(pid, 13, flags: vo, globalVoiceOver: true) // VO-W reads the current word.
        spin(0.3)
        try frontmost(pid)
        let phrase = try scopedVoiceOverPhrase(area)
        let readAlpha = phrase.localizedCaseInsensitiveContains("alpha")
        record("voiceover_read_word", readAlpha ? "PASS" : "FAIL", ["expectedMatched": readAlpha, "phraseSHA256": digest(phrase)])
        try require(readAlpha, "VoiceOver did not report the expected synthetic word")

        try key(pid, 124, flags: vo, globalVoiceOver: true)
        try key(pid, 13, flags: vo, globalVoiceOver: true)
        spin(0.2)
        let nextPhrase = try scopedVoiceOverPhrase(area)
        record("voiceover_word_navigation", nextPhrase.localizedCaseInsensitiveContains("beta") ? "PASS" : "FAIL", ["phraseSHA256": digest(nextPhrase)])
        try require(nextPhrase.localizedCaseInsensitiveContains("beta"), "VoiceOver did not navigate to the expected next word")

        try select(area, NSRange(location: 0, length: 0))
        for _ in 0..<5 { try key(pid, 124, flags: .maskShift) }
        let selected = ax(area, kAXSelectedTextAttribute) as? String
        record("voiceover_native_selection", selected == "alpha" ? "PASS" : "FAIL")
        try require(selected == "alpha", "Selection while VoiceOver was enabled did not match the expected word")
        try typeText(pid, "delta")
        let replaced = "delta" + String(fixture.dropFirst(5))
        try require(wait { text(area) == replaced }, "Native replacement while VoiceOver was enabled failed")
        record("voiceover_native_replace", "PASS")
        try key(pid, 6, flags: .maskCommand)
        try require(wait { text(area) == fixture }, "Native undo while VoiceOver was enabled failed")
        record("voiceover_native_undo", "PASS")
        try voiceOverOffscreen(area)
        record("voiceover_interaction", "PASS", ["scope": "Actual macOS VoiceOver reported expected synthetic words and offscreen content while native selection, replacement and undo remained editable", "audioCaptured": false])
    }

    func acceptVoiceOverWelcome() throws -> Bool {
        for setup in NSRunningApplication.runningApplications(withBundleIdentifier: "com.apple.VoiceOverQuickstart") {
            let setupPID = setup.processIdentifier
            if !quickstartBefore.contains(setupPID) { testQuickstart.insert(setupPID) }
            let root = AXUIElementCreateApplication(setupPID)
            guard let windows = ax(root, kAXWindowsAttribute) as? [AXUIElement] else { continue }
            for window in windows {
                // Start from setup windows, never the application menu bar.
                let descendants = ownedDescendants(window, pid: setupPID, limit: 100)
                let welcome = descendants.contains {
                    (ax($0, kAXRoleAttribute) as? String) == kAXStaticTextRole
                        && (ax($0, kAXValueAttribute) as? String) == "Welcome to VoiceOver"
                }
                guard welcome, let button = descendants.first(where: {
                    (ax($0, kAXRoleAttribute) as? String) == kAXButtonRole
                        && (ax($0, kAXTitleAttribute) as? String) == "Use VoiceOver"
                }) else { continue }
                let checkbox = descendants.first {
                    (ax($0, kAXRoleAttribute) as? String) == kAXCheckBoxRole
                        && (ax($0, kAXTitleAttribute) as? String) == "Do not show this message again"
                }
                let originalValue = checkbox.flatMap { ax($0, kAXValueAttribute) as? NSNumber }?.intValue
                AXUIElementSetMessagingTimeout(button, 0.5)
                let result = AXUIElementPerformAction(button, kAXPressAction as CFString)
                welcomeAction = [
                    "setupPID": setupPID, "button": "Use VoiceOver", "pressCode": result.rawValue,
                    "doNotShowAgainBefore": originalValue ?? -1, "checkboxChanged": false,
                    "limits": "A press timeout may occur while the service starts; the enabled flag is verified separately",
                ]
                spin(0.3)
                return true
            }
        }
        return false
    }

    func voiceOverOffscreen(_ area: AXUIElement) throws {
        let paragraphs = ownedDescendants(area, pid: pid).filter { (ax($0, kAXRoleAttribute) as? String) == kAXGroupRole }
        guard let last = paragraphs.last, let before = frame(last),
              let window = (ax(AXUIElementCreateApplication(pid), kAXWindowsAttribute) as? [AXUIElement])?.first,
              let windowFrame = frame(window) else {
            throw ProbeFailure.failed("Native offscreen paragraph/window bounds are unavailable during VoiceOver interaction")
        }
        let wasOffscreen = !windowFrame.intersects(before)
        let vo: CGEventFlags = [.maskControl, .maskAlternate]
        // Apple's VO-End text command moves to the end and scrolls as needed.
        // The fixture deliberately ends with a nonempty sentinel paragraph.
        try key(pid, 119, flags: vo, globalVoiceOver: true)
        try key(pid, 37, flags: vo, globalVoiceOver: true) // VO-L reads the line.
        spin(0.3)
        let phrase = try scopedVoiceOverPhrase(area)
        let nowVisible = frame(last).map { windowFrame.intersects($0) } ?? false
        let expectedMatched = phrase.localizedCaseInsensitiveContains("final sentinel")
        let passed = wasOffscreen && nowVisible && expectedMatched
        record("voiceover_offscreen_navigation", passed ? "PASS" : "FAIL", [
            "wasOffscreen": wasOffscreen, "becameVisible": nowVisible, "expectedMatched": expectedMatched,
            "beforeY": before.minY, "afterY": frame(last)?.minY ?? -1, "phraseSHA256": digest(phrase),
        ])
        try require(passed, "VoiceOver did not navigate to and report the expected offscreen sentinel")
    }

    func scopedVoiceOverPhrase(_ area: AXUIElement) throws -> String {
        try frontmost(pid)
        guard let app, let ownWindow = (ax(AXUIElementCreateApplication(app.processIdentifier), kAXWindowsAttribute) as? [AXUIElement])?.first,
              let windowFrame = frame(ownWindow), let editorFrame = frame(area) else {
            throw ProbeFailure.failed("Cannot scope VoiceOver output to our own visible editor")
        }
        // Read only numerical cursor bounds first. Do not inspect speech output
        // if VoiceOver is pointing somewhere outside our own visible text area.
        let cursor = try scriptBounds(appleScript("tell application \"VoiceOver\" to get bounds of vo cursor"))
        try require(cursor.width > 0 && cursor.height > 0 && windowFrame.intersection(editorFrame).intersects(cursor),
                    "Refusing VoiceOver output because its cursor is outside our visible editor")
        return try appleScript("tell application \"VoiceOver\" to get content of last phrase")
    }

    func restore() {
        guard !restorationStarted else { return }
        restorationStarted = true
        if let area, let original = priorText {
            AXUIElementSetMessagingTimeout(area, 0.5)
            let status = AXUIElementSetAttributeValue(area, kAXValueAttribute as CFString, original as CFString)
            textRestored = status == .success && wait { text(area) == original }
            if status == .success && !textRestored {
                // A real native action can be queued without waking an idle
                // renderer. Preserve the failed qualification evidence, but
                // wake this disposable app with an exact-PID harmless caret
                // event to finish restoring its source. Never count recovery
                // as proof that AX itself woke the app.
                try? focus()
                try? key(pid, 124)
                textRestored = wait { text(area) == original }
                record("restoration_required_native_wakeup", textRestored ? "PARTIAL" : "FAIL")
            }
            if textRestored, let old = priorRange {
                do {
                    try select(area, NSRange(location: old.location, length: old.length))
                    record("demo_selection_restored", "PASS")
                } catch {
                    record("demo_selection_restored", "FAIL", ["reason": String(describing: error)])
                }
            }
            record("demo_source_restored", textRestored ? "PASS" : "FAIL")
        }
        do {
            if voiceOverStarted && !voiceOverBefore {
                // Restore the scripting option even if the service stopped.
                try configureVoiceOver(false)
            }
            // Close only setup processes first observed after this probe's
            // normal enable attempt. Never terminate pre-existing setup apps
            // or use process termination as proof of the service's state.
            for setup in NSRunningApplication.runningApplications(withBundleIdentifier: "com.apple.VoiceOverQuickstart")
                where testQuickstart.contains(setup.processIdentifier) && !quickstartBefore.contains(setup.processIdentifier) {
                _ = setup.terminate()
            }
            let deadline = Date().addingTimeInterval(14)
            repeat {
                voiceOverAfterFresh = try freshVoiceOverEnabled()
                if voiceOverAfterFresh == voiceOverBefore { break }
                spin(1)
            } while Date() < deadline
            voiceOverRestored = voiceOverAfterFresh == voiceOverBefore
        } catch {
            voiceOverRestored = false
            record("voiceover_restoration_status", "FAIL", ["reason": String(describing: error)])
        }
        record("voiceover_state_restored", voiceOverRestored ? "PASS" : "FAIL")
    }

    func save() throws {
        let failed = checks.contains { ($0["status"] as? String) == "FAIL" }
        let partial = checks.contains { ["PARTIAL", "NOT_RUN"].contains($0["status"] as? String ?? "") }
        let report: [String: Any] = [
            "schema": 1, "kind": "standalone-native-accessibility-probe", "pid": pid,
            "os": ProcessInfo.processInfo.operatingSystemVersionString,
            "AXTrusted": AXIsProcessTrusted(), "postEventAllowed": CGPreflightPostEventAccess(),
            "currentInputSource": currentInputID() ?? "<unavailable>",
            "voiceOverBefore": voiceOverBefore, "voiceOverAfter": NSWorkspace.shared.isVoiceOverEnabled,
            "voiceOverAfterFresh": voiceOverAfterFresh as Any? ?? NSNull(),
            "overall": failed ? "FAIL" : (partial ? "PARTIAL" : "PASS"), "checks": checks,
            "limits": ["Programmatic native AX/CGEvent interaction is not physical keyboard input", "No audio is captured or evaluated; VoiceOver results concern its scoped reported text and actual editable interactions", "No clipboard contents are accessed", "Demo source/selection are restored; test history is ephemeral in this disposable demo", "VoiceOver start/restoration require independent process status; the enabled flag alone does not qualify screen-reader interaction"],
        ]
        let data = try JSONSerialization.data(withJSONObject: report, options: [.prettyPrinted, .sortedKeys])
        if let previous = try? Data(contentsOf: output) {
            let archive = output.deletingLastPathComponent().appendingPathComponent("platform-ax-prior-\(UUID().uuidString).json")
            try previous.write(to: archive, options: .atomic)
            print("NATIVE_ACCESSIBILITY_PREVIOUS_REPORT \(archive.path)")
        }
        try data.write(to: output, options: .atomic)
        print("NATIVE_ACCESSIBILITY_REPORT \(output.path)")
    }
}

func arguments() throws -> (pid_t, URL, Bool) {
    var args = Array(CommandLine.arguments.dropFirst())
    if args == ["--help"] {
        print("Usage: swift tools/accessibility_probe.swift --pid DEMO_PID --output /absolute/project/docs/evidence/platform-ax.json [--with-voiceover]")
        print("Run only against a fresh disposable demo after explicit temporary native-test setup approval. The US source must already be selected. No input source or permission setting is changed.")
        exit(0)
    }
    var pid: pid_t?
    var output: URL?
    var withVoiceOver = false
    while !args.isEmpty {
        let name = args.removeFirst()
        if name == "--with-voiceover" { withVoiceOver = true; continue }
        guard !args.isEmpty else { throw ProbeFailure.invalidArguments }
        let value = args.removeFirst()
        switch name {
        case "--pid": pid = Int32(value)
        case "--output":
            guard value.hasPrefix("/"), URL(fileURLWithPath: value).lastPathComponent.hasPrefix("platform-"), value.hasSuffix(".json") else {
                throw ProbeFailure.invalidArguments
            }
            output = URL(fileURLWithPath: value)
        default: throw ProbeFailure.invalidArguments
        }
    }
    guard let pid, pid > 1, let output else { throw ProbeFailure.invalidArguments }
    return (pid, output, withVoiceOver)
}

do {
    let (pid, output, withVoiceOver) = try arguments()
    let probe = Probe(pid: pid, output: output, withVoiceOver: withVoiceOver)
    let previousINT = Darwin.signal(SIGINT, SIG_IGN)
    let previousTERM = Darwin.signal(SIGTERM, SIG_IGN)
    var interruptionSources: [DispatchSourceSignal] = []
    for signal in [SIGINT, SIGTERM] {
        let source = DispatchSource.makeSignalSource(signal: signal, queue: .main)
        source.setEventHandler {
            probe.interruptedExit = 128 + signal
            probe.record("probe_interrupted", "FAIL", ["signal": signal])
            if !probe.restorationStarted {
                probe.restore()
                try? probe.save()
                exit(128 + signal)
            }
        }
        source.resume()
        interruptionSources.append(source)
    }
    defer {
        interruptionSources.forEach { $0.cancel() }
        Darwin.signal(SIGINT, previousINT)
        Darwin.signal(SIGTERM, previousTERM)
    }
    do { try probe.run() }
    catch { probe.record("probe_execution", "FAIL", ["reason": String(describing: error)]) }
    probe.restore()
    try probe.save()
    if let interrupted = probe.interruptedExit { exit(interrupted) }
    if probe.checks.contains(where: { ($0["status"] as? String) == "FAIL" }) { exit(1) }
} catch {
    fputs("NATIVE_ACCESSIBILITY_ARGUMENT_ERROR \(error)\n", stderr)
    exit(2)
}
