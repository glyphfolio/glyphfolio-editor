#!/usr/bin/env swift
// Opt-in native clipboard/UI proof for the exact fresh standalone demo only.
// Prior pasteboard representations remain only in memory and are never logged.
// Do not run concurrently with user copying or another native input probe.
import AppKit
import ApplicationServices
import CoreGraphics
import Foundation
import Darwin
import Dispatch

enum ClipboardUIError: Error, CustomStringConvertible {
    case failed(String)
    case blocked(String)
    var description: String {
        switch self { case .failed(let message), .blocked(let message): return message }
    }
    var status: String {
        switch self { case .failed: return "FAIL"; case .blocked: return "PARTIAL" }
    }
}

func require(_ condition: Bool, _ message: String) throws {
    if !condition { throw ClipboardUIError.failed(message) }
}

func spin(_ seconds: Double) {
    let deadline = Date().addingTimeInterval(seconds)
    while Date() < deadline {
        RunLoop.current.run(until: min(deadline, Date().addingTimeInterval(0.025)))
    }
}

func wait(_ seconds: Double = 3, _ predicate: () -> Bool) -> Bool {
    let deadline = Date().addingTimeInterval(seconds)
    repeat {
        if predicate() { return true }
        spin(0.025)
    } while Date() < deadline
    return predicate()
}

func ax(_ element: AXUIElement, _ attribute: String) -> CFTypeRef? {
    var value: CFTypeRef?
    guard AXUIElementCopyAttributeValue(element, attribute as CFString, &value) == .success else { return nil }
    return value
}

func axElement(_ value: CFTypeRef?) -> AXUIElement? {
    guard let value, CFGetTypeID(value) == AXUIElementGetTypeID() else { return nil }
    return unsafeBitCast(value, to: AXUIElement.self)
}

func axBounds(_ element: AXUIElement) -> CGRect? {
    guard let rawPosition = ax(element, kAXPositionAttribute),
          let rawSize = ax(element, kAXSizeAttribute),
          CFGetTypeID(rawPosition) == AXValueGetTypeID(), CFGetTypeID(rawSize) == AXValueGetTypeID() else { return nil }
    var point = CGPoint.zero
    var size = CGSize.zero
    guard AXValueGetValue(unsafeBitCast(rawPosition, to: AXValue.self), .cgPoint, &point),
          AXValueGetValue(unsafeBitCast(rawSize, to: AXValue.self), .cgSize, &size),
          point.x.isFinite, point.y.isFinite, size.width.isFinite, size.height.isFinite,
          size.width > 0, size.height > 0 else { return nil }
    return CGRect(origin: point, size: size)
}

func text(_ element: AXUIElement) -> String? { ax(element, kAXValueAttribute) as? String }

func set(_ element: AXUIElement, _ attribute: String, _ value: CFTypeRef) throws {
    let result = AXUIElementSetAttributeValue(element, attribute as CFString, value)
    try require(result == .success, "Native AX setter was rejected for \(attribute), code=\(result.rawValue)")
}

func selectedRange(_ element: AXUIElement) -> CFRange? {
    guard let raw = ax(element, kAXSelectedTextRangeAttribute), CFGetTypeID(raw) == AXValueGetTypeID() else { return nil }
    var range = CFRange()
    return AXValueGetValue(unsafeBitCast(raw, to: AXValue.self), .cfRange, &range) ? range : nil
}

func select(_ element: AXUIElement, _ requested: CFRange) throws {
    var requested = requested
    guard let value = AXValueCreate(.cfRange, &requested) else { throw ClipboardUIError.failed("Cannot construct a native UTF-16 selection") }
    try set(element, kAXSelectedTextRangeAttribute, value)
    try require(wait { selectedRange(element).map { $0.location == requested.location && $0.length == requested.length } ?? false },
                "Native UTF-16 selection was not acknowledged")
}

func descendants(_ root: AXUIElement, pid: pid_t) -> [AXUIElement] {
    var queue = [root]
    var output: [AXUIElement] = []
    var visited: [AXUIElement] = []
    let deadline = Date().addingTimeInterval(5)
    while let element = queue.popLast(), output.count < 4000, Date() < deadline {
        AXUIElementSetMessagingTimeout(element, 0.5)
        var owner: pid_t = 0
        guard AXUIElementGetPid(element, &owner) == .success, owner == pid,
              !visited.contains(where: { CFEqual($0, element) }) else { continue }
        visited.append(element)
        output.append(element)
        if let children = ax(element, kAXChildrenAttribute) as? [AXUIElement] { queue.append(contentsOf: children.reversed()) }
    }
    return output
}

struct Arguments {
    let pid: pid_t
    let output: URL
    static func parse() throws -> Arguments? {
        var args = Array(CommandLine.arguments.dropFirst())
        if args == ["--help"] {
            print("Usage: swift tools/clipboard_ui_probe.swift --pid DEMO_PID --output /absolute/platform-clipboard.json")
            print("Uses native Cmd-C/X/V/Z in the fresh synthetic standalone demo, preserves all pasteboard representations only in memory, then restores source/selection and clipboard bytes. Never run concurrently with user copying or another native probe.")
            return nil
        }
        var pid: pid_t?
        var output: URL?
        while !args.isEmpty {
            let argument = args.removeFirst()
            guard !args.isEmpty else { throw ClipboardUIError.failed("Missing argument value") }
            let value = args.removeFirst()
            switch argument {
            case "--pid":
                guard let parsed = Int32(value), parsed > 1 else { throw ClipboardUIError.failed("Expected a standalone user-process PID") }
                pid = parsed
            case "--output":
                guard value.hasPrefix("/"), URL(fileURLWithPath: value).pathExtension == "json" else {
                    throw ClipboardUIError.failed("Output must be an absolute JSON path")
                }
                output = URL(fileURLWithPath: value).standardizedFileURL
            default: throw ClipboardUIError.failed("Unknown argument: \(argument)")
            }
        }
        guard let pid, let output else { throw ClipboardUIError.failed("Expected --pid and --output; see --help") }
        return Arguments(pid: pid, output: output)
    }
}

final class ClipboardUIProbe {
    let arguments: Arguments
    let board = NSPasteboard.general
    var checks: [[String: Any]] = []
    var currentCheck = "setup"
    var app: NSRunningApplication?
    var executable: URL?
    var launched: Date?
    var root: AXUIElement?
    var area: AXUIElement?
    var priorText: String?
    var priorRange: CFRange?
    var sourceTouched = false
    var selectionTouched = false
    var sourceRestored = false
    var selectionRestored = false
    var clipboardCaptured = false
    var capturedChange = 0
    var priorItems: [[(NSPasteboard.PasteboardType, Data)]] = []
    var ownedChange: Int?
    var clipboardRestored = false
    var clipboardOwnerChanged = false
    var commandHeld = false
    var nativeShortcutsPosted = 0
    var nativeFocusClickPosted = false
    var operationsPassed = false
    var blocked = false
    var restorationStarted = false
    var interruptedExit: Int32?

    init(_ arguments: Arguments) { self.arguments = arguments }

    func record(_ name: String, _ status: String, _ detail: [String: Any] = [:]) {
        checks.append(["check": name, "status": status, "detail": detail])
        print("NATIVE_CLIPBOARD_UI \(name)=\(status)")
        fflush(stdout)
    }

    func verifyProcess() throws {
        guard let expected = executable,
              let current = NSRunningApplication(processIdentifier: arguments.pid),
              current.executableURL?.standardizedFileURL.resolvingSymlinksInPath() == expected,
              current.launchDate == launched, !current.isTerminated else {
            throw ClipboardUIError.failed("Standalone demo identity changed or exited")
        }
    }

    func frontmost() throws {
        try verifyProcess()
        try require(NSWorkspace.shared.frontmostApplication?.processIdentifier == arguments.pid && app?.isActive == true,
                    "User focus left the standalone demo; native events aborted")
    }

    func focusedTarget() -> Bool {
        guard let root, let area,
              let window = axElement(ax(root, kAXFocusedWindowAttribute)) else { return false }
        let focused = axElement(ax(root, kAXFocusedUIElementAttribute))
        if let focused {
            guard (ax(focused, kAXRoleAttribute) as? String) == kAXTextAreaRole,
                  CFEqual(focused, area) else { return false }
        } else if !nativeFocusClickPosted {
            return false
        }
        var areaOwner: pid_t = 0
        var windowOwner: pid_t = 0
        // Missing application focused-element reporting is an AX gate concern.
        // Here the native click, exact owned key window, selection, and final
        // canonical clipboard/edit assertions jointly verify input routing.
        return AXUIElementGetPid(area, &areaOwner) == .success && areaOwner == arguments.pid
            && AXUIElementGetPid(window, &windowOwner) == .success && windowOwner == arguments.pid
    }

    func verifyFocusedTarget() throws {
        try frontmost()
        try require(focusedTarget(), "The verified standalone text area lost native keyboard focus; events aborted")
    }

    func focusNativeTextArea() throws {
        guard let app, let root, let area, let original = priorText else {
            throw ClipboardUIError.failed("Cannot focus the preserved own-demo text area")
        }
        app.activate(options: [.activateAllWindows])
        _ = AXUIElementSetAttributeValue(root, kAXFrontmostAttribute as CFString, kCFBooleanTrue)
        if let window = axElement(ax(root, kAXFocusedWindowAttribute)) {
            _ = AXUIElementPerformAction(window, kAXRaiseAction as CFString)
            _ = AXUIElementSetAttributeValue(window, kAXMainAttribute as CFString, kCFBooleanTrue)
            _ = AXUIElementSetAttributeValue(window, kAXFocusedAttribute as CFString, kCFBooleanTrue)
        }
        try require(wait(5) {
            NSWorkspace.shared.frontmostApplication?.processIdentifier == self.arguments.pid && app.isActive
        }, "Standalone demo did not become frontmost")
        try set(area, kAXFocusedAttribute, kCFBooleanTrue)
        try frontmost()
        try require(text(area) == original, "Own-demo source changed before native focus setup")
        guard let bounds = axBounds(area) else { throw ClipboardUIError.failed("Cannot verify own-demo text area bounds") }
        let point = CGPoint(x: bounds.minX + min(24, bounds.width / 2),
                            y: bounds.minY + min(24, bounds.height / 2))
        guard let down = CGEvent(mouseEventSource: nil, mouseType: .leftMouseDown, mouseCursorPosition: point, mouseButton: .left),
              let up = CGEvent(mouseEventSource: nil, mouseType: .leftMouseUp, mouseCursorPosition: point, mouseButton: .left) else {
            throw ClipboardUIError.failed("Cannot construct own-demo native focus click")
        }
        // AXFocused can report success while the native window still lacks an
        // input context. Click only the verified frontmost synthetic text area,
        // then restore its captured selection before changing the fixture.
        selectionTouched = true
        try frontmost()
        down.post(tap: .cghidEventTap)
        var mouseReleased = false
        defer {
            // If focus changes between the pair, release only on our verified
            // demo instead of sending any further global mouse input.
            if !mouseReleased, (try? verifyProcess()) != nil { up.postToPid(arguments.pid) }
        }
        spin(0.05)
        try frontmost()
        up.post(tap: .cghidEventTap)
        mouseReleased = true
        nativeFocusClickPosted = true
        try require(wait(5) { self.focusedTarget() }, "Native click did not focus the verified standalone text area")
        try verifyFocusedTarget()
        try require(text(area) == original, "Native focus click unexpectedly changed own-demo source")
        guard let priorRange else { throw ClipboardUIError.failed("Captured own-demo selection is unavailable") }
        try select(area, priorRange)
        record("native_input_target", "PASS", ["physicalClickPosted": true, "originalSelectionRestoredBeforeFixture": true,
            "applicationFocusedElementAvailable": ax(root, kAXFocusedUIElementAttribute) != nil])
    }

    func setup() throws {
        currentCheck = "native_permissions"
        guard AXIsProcessTrusted() && CGPreflightPostEventAccess() else {
            throw ClipboardUIError.blocked("Native accessibility/event-posting permission is unavailable; no test input posted")
        }
        record(currentCheck, "PASS")
        currentCheck = "exact_standalone_target"
        guard let app = NSRunningApplication(processIdentifier: arguments.pid), let url = app.executableURL else {
            throw ClipboardUIError.failed("Standalone demo process is unavailable")
        }
        let resolved = url.standardizedFileURL.resolvingSymlinksInPath()
        let project = URL(fileURLWithPath: #filePath).standardizedFileURL.deletingLastPathComponent()
            .deletingLastPathComponent().resolvingSymlinksInPath()
        try require(resolved.lastPathComponent == "glyphfolio-editor-demo" && resolved.path.hasPrefix(project.appendingPathComponent("target").path + "/"),
                    "Refusing an executable outside this workspace's standalone demo target")
        self.app = app
        executable = resolved
        launched = app.launchDate
        let root = AXUIElementCreateApplication(arguments.pid)
        self.root = root
        AXUIElementSetMessagingTimeout(root, 1)
        try require(wait(5) {
            self.area = descendants(root, pid: self.arguments.pid).first { (ax($0, kAXRoleAttribute) as? String) == kAXTextAreaRole }
            return self.area != nil
        }, "The standalone native text area is unavailable")
        guard let area, let original = text(area), let selected = selectedRange(area) else {
            throw ClipboardUIError.failed("Cannot preserve the own-demo source/selection")
        }
        try require(original.hasPrefix("# glyphfolio-editor\n") && original.contains("feasibility"),
                    "Refusing to replace anything other than the known fresh synthetic demo source")
        priorText = original
        priorRange = selected
        record(currentCheck, "PASS")

        currentCheck = "clipboard_capture_before_mutation"
        capturedChange = board.changeCount
        var bytes = 0
        for item in board.pasteboardItems ?? [] {
            var saved: [(NSPasteboard.PasteboardType, Data)] = []
            for type in item.types {
                guard let data = item.data(forType: type) else {
                    throw ClipboardUIError.blocked("Cannot preserve a pasteboard representation; no test mutation performed")
                }
                bytes += data.count
                guard bytes <= 32 * 1024 * 1024 else {
                    throw ClipboardUIError.blocked("Clipboard preservation exceeds the 32-MiB limit; no test mutation performed")
                }
                saved.append((type, data))
            }
            priorItems.append(saved)
        }
        try require(board.changeCount == capturedChange, "Clipboard changed while preserving it; probe cancelled before mutation")
        clipboardCaptured = true
        record(currentCheck, "PASS")

        currentCheck = "native_input_target"
        try focusNativeTextArea()
        currentCheck = "synthetic_fixture_selection"
        try verifyFocusedTarget()
        sourceTouched = true
        try set(area, kAXValueAttribute, "left TARGET right" as CFString)
        try require(wait { text(area) == "left TARGET right" }, "Synthetic fixture was not acknowledged")
        try select(area, CFRange(location: 5, length: 6))
        try require((ax(area, kAXSelectedTextAttribute) as? String) == "TARGET", "Selected synthetic fixture text disagrees")
        try require(board.changeCount == capturedChange, "Clipboard owner changed before native copy")
        record(currentCheck, "PASS")
    }

    func ensureClipboardOwner() throws {
        let expected = ownedChange ?? capturedChange
        if board.changeCount != expected {
            clipboardOwnerChanged = true
            throw ClipboardUIError.failed("Clipboard owner changed during the probe; refusing further clipboard operations")
        }
    }

    func post(_ event: CGEvent) throws {
        try verifyFocusedTarget()
        event.postToPid(arguments.pid)
    }

    func releaseCommand() {
        guard commandHeld, let up = CGEvent(keyboardEventSource: nil, virtualKey: 55, keyDown: false) else { return }
        up.type = .flagsChanged
        up.flags = []
        // Release only our modifier on the exact still-owned demo, even if user
        // focus moved. No global modifier/input event is sent to another app.
        if (try? verifyProcess()) != nil { up.postToPid(arguments.pid) }
        commandHeld = false
        spin(0.04)
    }

    func shortcut(_ code: CGKeyCode, _ letter: UniChar) throws {
        try verifyFocusedTarget()
        try ensureClipboardOwner()
        guard let modifierDown = CGEvent(keyboardEventSource: nil, virtualKey: 55, keyDown: true),
              let keyDown = CGEvent(keyboardEventSource: nil, virtualKey: code, keyDown: true),
              let keyUp = CGEvent(keyboardEventSource: nil, virtualKey: code, keyDown: false) else {
            throw ClipboardUIError.failed("Cannot construct a native clipboard shortcut")
        }
        modifierDown.type = .flagsChanged
        modifierDown.flags = .maskCommand
        keyDown.flags = .maskCommand
        keyUp.flags = .maskCommand
        // Supply only the shortcut's Latin key equivalent, never clipboard text.
        // The separate flagsChanged events ensure winit receives real modifier
        // transitions; merely putting Command on the letter event is insufficient.
        var equivalent = letter
        keyDown.keyboardSetUnicodeString(stringLength: 1, unicodeString: &equivalent)
        keyUp.keyboardSetUnicodeString(stringLength: 1, unicodeString: &equivalent)
        try post(modifierDown)
        commandHeld = true
        defer { releaseCommand() }
        spin(0.06)
        try post(keyDown)
        try post(keyUp)
        nativeShortcutsPosted += 1
        spin(0.08)
    }

    func acknowledgeSyntheticClipboardWrite() throws {
        let previous = ownedChange ?? capturedChange
        let acknowledged = wait {
            let count = self.board.changeCount
            if count != previous && self.board.string(forType: .string) == "TARGET" {
                self.ownedChange = count
                return true
            }
            return false
        }
        if !acknowledged && board.changeCount != previous { clipboardOwnerChanged = true }
        try require(acknowledged, "Native clipboard publication did not produce the exact synthetic selection")
        try ensureClipboardOwner()
    }

    func publishSyntheticPayload(_ payload: String) throws {
        try verifyFocusedTarget()
        try ensureClipboardOwner()
        let markerType = NSPasteboard.PasteboardType("org.glyphfolio.editor.native-clipboard-probe-owner")
        let marker = Data(UUID().uuidString.utf8)
        let bytes = Data(payload.utf8)
        let item = NSPasteboardItem()
        try require(item.setData(bytes, forType: .string) && item.setData(marker, forType: markerType),
                    "Cannot prepare the synthetic Markdown clipboard item")
        // Track our clear immediately, then acknowledge the publication only
        // when our private random marker and exact synthetic bytes are present.
        // No prior clipboard payload or representation name is written to logs.
        let cleared = board.clearContents()
        ownedChange = cleared
        try ensureClipboardOwner()
        let published = board.writeObjects([item])
        let observed = board.changeCount
        let items = board.pasteboardItems ?? []
        let ownItem = items.count == 1
            && items[0].data(forType: markerType) == marker
            && items[0].data(forType: .string) == bytes
            && board.changeCount == observed
        if ownItem { ownedChange = observed }
        else if observed != cleared { clipboardOwnerChanged = true }
        try require(published && ownItem, "Synthetic Markdown clipboard publication was not acknowledged; no paste posted")
        try ensureClipboardOwner()
    }

    func runOperations() throws {
        try setup()
        guard let area else { throw ClipboardUIError.failed("Standalone text area disappeared") }
        currentCheck = "native_command_copy"
        try shortcut(8, 99) // Cmd-C.
        try acknowledgeSyntheticClipboardWrite()
        try require(text(area) == "left TARGET right", "Copy unexpectedly changed the document")
        record(currentCheck, "PASS")

        currentCheck = "native_command_cut_acknowledged"
        try shortcut(7, 120) // Cmd-X.
        try acknowledgeSyntheticClipboardWrite()
        try require(wait { text(area) == "left  right" }, "Acknowledged cut did not remove exactly the selected range")
        record(currentCheck, "PASS")

        currentCheck = "native_command_paste"
        try shortcut(9, 118) // Cmd-V.
        try require(wait { text(area) == "left TARGET right" }, "Native paste did not restore the exact synthetic selection")
        try ensureClipboardOwner()
        record(currentCheck, "PASS")

        currentCheck = "undo_paste_is_discrete"
        try shortcut(6, 122) // Cmd-Z.
        try require(wait { text(area) == "left  right" }, "First undo did not reverse exactly the paste")
        try ensureClipboardOwner()
        record(currentCheck, "PASS")

        currentCheck = "undo_cut_is_discrete"
        try shortcut(6, 122)
        try require(wait { text(area) == "left TARGET right" }, "Second undo did not restore exactly the cut selection")
        try ensureClipboardOwner()
        record(currentCheck, "PASS")

        currentCheck = "synthetic_markdown_unicode_crlf_publication"
        let fixture = "left TARGET right"
        let payload = "**日本 😀**\r\nline\r\n"
        try verifyFocusedTarget()
        try set(area, kAXValueAttribute, fixture as CFString)
        try require(wait { text(area) == fixture }, "Synthetic Markdown paste fixture was not acknowledged")
        try select(area, CFRange(location: 5, length: 6))
        try require((ax(area, kAXSelectedTextAttribute) as? String) == "TARGET", "Synthetic Markdown paste selection disagrees")
        try publishSyntheticPayload(payload)
        record(currentCheck, "PASS", ["syntheticUTF8Bytes": payload.utf8.count, "crlfPairs": 2])

        currentCheck = "native_markdown_unicode_crlf_paste_exact"
        try shortcut(9, 118) // Cmd-V through the actual widget.
        let expectedBytes = Data(("left " + payload + " right").utf8)
        try require(wait { text(area).map { Data($0.utf8) == expectedBytes } ?? false },
                    "Native Markdown/Unicode paste did not preserve the exact UTF-8 payload including CRLF")
        try ensureClipboardOwner()
        record(currentCheck, "PASS", ["exactSourceUTF8": true, "crlfPairs": 2])

        currentCheck = "undo_markdown_unicode_crlf_paste_is_discrete"
        try shortcut(6, 122) // One Cmd-Z restores the selected fixture.
        let fixtureBytes = Data(fixture.utf8)
        try require(wait { text(area).map { Data($0.utf8) == fixtureBytes } ?? false },
                    "One undo did not restore the exact fixture after Markdown/Unicode CRLF paste")
        try ensureClipboardOwner()
        record(currentCheck, "PASS")
        operationsPassed = true
    }

    func restoreSource() {
        guard sourceTouched || selectionTouched, let area, let original = priorText, let selection = priorRange else { return }
        do {
            try verifyProcess()
            if sourceTouched { try set(area, kAXValueAttribute, original as CFString) }
            sourceRestored = wait { text(area) == original }
            try require(sourceRestored, "Original own-demo source was not restored")
            try select(area, selection)
            selectionRestored = selectedRange(area).map { $0.location == selection.location && $0.length == selection.length } ?? false
            record("own_source_selection_restoration", sourceRestored && selectionRestored ? "PASS" : "FAIL")
        } catch {
            record("own_source_selection_restoration", "FAIL", ["reason": String(describing: error)])
        }
    }

    func restoreClipboard() {
        guard clipboardCaptured else { return }
        guard let owned = ownedChange else {
            clipboardRestored = board.changeCount == capturedChange
            if !clipboardRestored { clipboardOwnerChanged = true }
            record("clipboard_representations_restoration", clipboardRestored ? "PASS" : "FAIL",
                   ["unchangedWithoutOwnedWrite": clipboardRestored, "skippedNewOwner": !clipboardRestored])
            return
        }
        guard !clipboardOwnerChanged && board.changeCount == owned else {
            clipboardOwnerChanged = true
            record("clipboard_representations_restoration", "FAIL", ["skippedNewOwner": true])
            return
        }
        let restored: [NSPasteboardItem] = priorItems.map { representations in
            let item = NSPasteboardItem()
            for (type, data) in representations { item.setData(data, forType: type) }
            return item
        }
        board.clearContents()
        let cleared = board.changeCount
        guard board.changeCount == cleared else {
            clipboardOwnerChanged = true
            record("clipboard_representations_restoration", "FAIL", ["skippedNewOwner": true])
            return
        }
        let published = restored.isEmpty || board.writeObjects(restored)
        let restoredChange = board.changeCount
        let actual = board.pasteboardItems ?? []
        var same = published && actual.count == priorItems.count
        if same {
            for (index, item) in actual.enumerated() {
                let wanted = priorItems[index]
                same = same && Set(item.types) == Set(wanted.map { $0.0 })
                for (type, data) in wanted { same = same && item.data(forType: type) == data }
            }
        }
        same = same && board.changeCount == restoredChange
        clipboardRestored = same
        record("clipboard_representations_restoration", same ? "PASS" : "FAIL", ["allItemsTypesAndBytesEqual": same])
    }

    func restore() {
        guard !restorationStarted else { return }
        restorationStarted = true
        releaseCommand()
        restoreSource()
        restoreClipboard()
    }

    func run() -> Int32 {
        do { try runOperations() }
        catch {
            let failure = error as? ClipboardUIError
            blocked = failure?.status == "PARTIAL"
            record(currentCheck, failure?.status ?? "FAIL", ["reason": String(describing: error)])
        }
        restore()
        let restored = (!(sourceTouched || selectionTouched) || (sourceRestored && selectionRestored)) && (!clipboardCaptured || clipboardRestored)
        let passed = operationsPassed && restored && !clipboardOwnerChanged
        let status = passed ? "PASS" : (blocked && restored ? "PARTIAL" : "FAIL")
        let result: [String: Any] = [
            "kind": "native-clipboard-widget-proof", "pid": arguments.pid, "status": status,
            "checks": checks, "actualWidgetKeyboardPath": nativeShortcutsPosted > 0,
            "nativeFocusClickPosted": nativeFocusClickPosted,
            "nativeShortcutsPosted": nativeShortcutsPosted, "mockClipboardUsed": false,
            "sourceRestored": sourceRestored, "selectionRestored": selectionRestored,
            "clipboardCaptured": clipboardCaptured, "clipboardRepresentationsRestored": clipboardRestored,
            "newClipboardOwnerObserved": clipboardOwnerChanged, "clipboardPayloadLogged": false,
            "inputSourcesChanged": false, "voiceOverChanged": false,
        ]
        do {
            let data = try JSONSerialization.data(withJSONObject: result, options: [.prettyPrinted, .sortedKeys])
            try data.write(to: arguments.output, options: .atomic)
        } catch {
            fputs("NATIVE_CLIPBOARD_UI evidence_write=FAIL\n", stderr)
            return 2
        }
        print("NATIVE_CLIPBOARD_UI result=\(status) sourceRestored=\(sourceRestored) selectionRestored=\(selectionRestored) clipboardRestored=\(clipboardRestored)")
        return passed ? 0 : (status == "PARTIAL" ? 3 : 1)
    }
}

do {
    if let arguments = try Arguments.parse() {
        let probe = ClipboardUIProbe(arguments)
        let oldINT = Darwin.signal(SIGINT, SIG_IGN)
        let oldTERM = Darwin.signal(SIGTERM, SIG_IGN)
        var handlers: [DispatchSourceSignal] = []
        for number in [SIGINT, SIGTERM] {
            let source = DispatchSource.makeSignalSource(signal: number, queue: .main)
            source.setEventHandler {
                probe.interruptedExit = 128 + number
                if !probe.restorationStarted {
                    probe.restore()
                    exit(128 + number)
                }
            }
            source.resume()
            handlers.append(source)
        }
        let status = probe.run()
        handlers.forEach { $0.cancel() }
        Darwin.signal(SIGINT, oldINT)
        Darwin.signal(SIGTERM, oldTERM)
        exit(probe.interruptedExit ?? status)
    }
} catch {
    fputs("NATIVE_CLIPBOARD_UI arguments=FAIL \(error)\n", stderr)
    exit(64)
}
