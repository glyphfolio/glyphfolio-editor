#!/usr/bin/env swift
// Guarded scan-code transport for the exact standalone synthetic demo only.
// PID transport is the default; opt-in HID transport exercises native IMK.
// A native click focuses the verified frontmost synthetic text area. No input
// source, VoiceOver, TCC, clipboard, or other application's text is changed.
import AppKit
import ApplicationServices
import CoreGraphics
import Foundation
import Darwin

enum KeyProbeError: Error, CustomStringConvertible {
    case failed(String)
    var description: String {
        switch self { case .failed(let message): return message }
    }
}

func require(_ condition: Bool, _ message: String) throws {
    if !condition { throw KeyProbeError.failed(message) }
}

func spin(_ seconds: Double) {
    let deadline = Date().addingTimeInterval(seconds)
    while Date() < deadline {
        RunLoop.current.run(until: min(deadline, Date().addingTimeInterval(0.025)))
    }
}

func wait(_ seconds: Double, _ predicate: () -> Bool) -> Bool {
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

func selectedRange(_ element: AXUIElement) -> CFRange? {
    guard let raw = ax(element, kAXSelectedTextRangeAttribute), CFGetTypeID(raw) == AXValueGetTypeID() else { return nil }
    var range = CFRange()
    guard AXValueGetValue(unsafeBitCast(raw, to: AXValue.self), .cfRange, &range) else { return nil }
    return range
}

func axBounds(_ element: AXUIElement) -> CGRect? {
    guard let rawPosition = ax(element, kAXPositionAttribute),
          let rawSize = ax(element, kAXSizeAttribute),
          CFGetTypeID(rawPosition) == AXValueGetTypeID(), CFGetTypeID(rawSize) == AXValueGetTypeID() else { return nil }
    var point = CGPoint.zero
    var size = CGSize.zero
    guard AXValueGetValue(unsafeBitCast(rawPosition, to: AXValue.self), .cgPoint, &point),
          AXValueGetValue(unsafeBitCast(rawSize, to: AXValue.self), .cgSize, &size) else { return nil }
    return CGRect(origin: point, size: size)
}

func rangeBounds(_ element: AXUIElement, _ range: CFRange) -> CGRect? {
    var range = range
    guard let value = AXValueCreate(.cfRange, &range) else { return nil }
    var raw: CFTypeRef?
    guard AXUIElementCopyParameterizedAttributeValue(element, kAXBoundsForRangeParameterizedAttribute as CFString, value, &raw) == .success,
          let raw, CFGetTypeID(raw) == AXValueGetTypeID() else { return nil }
    var bounds = CGRect.zero
    guard AXValueGetValue(unsafeBitCast(raw, to: AXValue.self), .cgRect, &bounds) else { return nil }
    return bounds
}

func rectJSON(_ rect: CGRect) -> [String: Double] {
    ["x": rect.origin.x, "y": rect.origin.y, "width": rect.width, "height": rect.height]
}

enum Mode: String { case latin, japanese, cancel, chinese, focus }

struct Arguments {
    var pid: pid_t
    var mode: Mode
    var startDelay: Double
    var candidateOutput: URL?
    var hid: Bool
    var exerciseViewport: Bool

    static func parse() throws -> Arguments? {
        var args = Array(CommandLine.arguments.dropFirst())
        if args == ["--help"] {
            print("Usage: swift tools/post_native_keys.swift --pid DEMO_PID --mode latin|japanese|cancel|chinese|focus [--transport pid|hid] [--exercise-viewport yes|no] [--start-delay 2.0] [--candidate-output /absolute/file.json]")
            print("Posts scan-code key pairs only while the exact standalone demo is frontmost. HID transport is opt-in for native IMK; these remain programmatic events. Input-source restoration is owned by the separate driver.")
            return nil
        }
        var pid: pid_t?
        var mode: Mode?
        var delay: Double = 2
        var output: URL?
        var hid = false
        var exerciseViewport = false
        while !args.isEmpty {
            let option = args.removeFirst()
            guard !args.isEmpty else { throw KeyProbeError.failed("Missing value for \(option)") }
            let value = args.removeFirst()
            switch option {
            case "--pid":
                guard let parsed = Int32(value), parsed > 1 else { throw KeyProbeError.failed("PID must name a user demo process") }
                pid = parsed
            case "--mode":
                guard let parsed = Mode(rawValue: value) else { throw KeyProbeError.failed("Unsupported mode") }
                mode = parsed
            case "--start-delay":
                guard let parsed = Double(value), parsed.isFinite, parsed >= 0, parsed <= 10 else {
                    throw KeyProbeError.failed("Start delay must be 0–10 seconds")
                }
                delay = parsed
            case "--candidate-output":
                guard value.hasPrefix("/"), URL(fileURLWithPath: value).pathExtension == "json" else {
                    throw KeyProbeError.failed("Candidate output must be an absolute .json path")
                }
                output = URL(fileURLWithPath: value).standardizedFileURL
            case "--transport":
                try require(value == "pid" || value == "hid", "Expected pid or hid transport")
                hid = value == "hid"
            case "--exercise-viewport":
                try require(value == "yes" || value == "no", "Expected yes or no viewport exercise")
                exerciseViewport = value == "yes"
            default: throw KeyProbeError.failed("Unknown option: \(option)")
            }
        }
        guard let pid, let mode else { throw KeyProbeError.failed("Expected --pid and --mode; see --help") }
        return Arguments(pid: pid, mode: mode, startDelay: delay, candidateOutput: output, hid: hid, exerciseViewport: exerciseViewport)
    }
}

final class NativeKeys {
    let arguments: Arguments
    let app: NSRunningApplication
    let executable: URL
    let launched: Date?
    let root: AXUIElement
    var area: AXUIElement?
    var posted = 0
    var nativeFocusClick = false

    init(_ arguments: Arguments) throws {
        self.arguments = arguments
        try require(CGPreflightPostEventAccess(), "Native event-posting permission is not granted; no events were posted")
        try require(AXIsProcessTrusted(), "Accessibility permission is required to verify the exact editor target")
        // Process.run can return before LaunchServices publishes the child.
        // Wait only for this exact PID; identity checks below still apply.
        _ = wait(5) {
            NSRunningApplication(processIdentifier: arguments.pid)?.executableURL != nil
        }
        guard let app = NSRunningApplication(processIdentifier: arguments.pid), let path = app.executableURL else {
            throw KeyProbeError.failed("The requested demo process is unavailable")
        }
        executable = path.standardizedFileURL.resolvingSymlinksInPath()
        let project = URL(fileURLWithPath: #filePath).standardizedFileURL
            .deletingLastPathComponent().deletingLastPathComponent().resolvingSymlinksInPath()
        let targetPrefix = project.appendingPathComponent("target", isDirectory: true).path + "/"
        try require(executable.lastPathComponent == "glyphfolio-editor-demo" && executable.path.hasPrefix(targetPrefix),
                    "Refusing an executable outside this workspace's standalone demo target")
        self.app = app
        launched = app.launchDate
        root = AXUIElementCreateApplication(arguments.pid)
        AXUIElementSetMessagingTimeout(root, 0.5)
    }

    func verifyProcess() throws {
        guard let current = NSRunningApplication(processIdentifier: arguments.pid),
              let path = current.executableURL?.standardizedFileURL.resolvingSymlinksInPath(),
              path == executable, current.launchDate == launched, !current.isTerminated else {
            throw KeyProbeError.failed("Demo identity changed or exited; refusing further events")
        }
    }

    func focusedArea() -> AXUIElement? {
        let applicationFocus = axElement(ax(root, kAXFocusedUIElementAttribute))
        // Some AppKit adapters return no application focused-UI element even
        // after our verified native click. That is separate AX gate evidence.
        // The isolated input probe can still target its own active key window;
        // final canonical source assertions determine whether editing occurred.
        let nativeTextFocus = nativeFocusClick ? findArea() : nil
        guard let element = applicationFocus ?? nativeTextFocus,
              (ax(element, kAXRoleAttribute) as? String) == kAXTextAreaRole else { return nil }
        var owner: pid_t = 0
        guard AXUIElementGetPid(element, &owner) == .success, owner == arguments.pid else { return nil }
        return element
    }

    func findArea() -> AXUIElement? {
        var pending = ax(root, kAXWindowsAttribute) as? [AXUIElement] ?? []
        pending.append(contentsOf: (ax(root, kAXChildrenAttribute) as? [AXUIElement] ?? []).filter {
            ax($0, kAXRoleAttribute) as? String == kAXWindowRole
        })
        var visited: [AXUIElement] = []
        let deadline = Date().addingTimeInterval(3)
        while let element = pending.popLast(), visited.count < 512, Date() < deadline {
            guard !visited.contains(where: { CFEqual($0, element) }) else { continue }
            visited.append(element)
            AXUIElementSetMessagingTimeout(element, 0.5)
            var owner: pid_t = 0
            guard AXUIElementGetPid(element, &owner) == .success, owner == arguments.pid else { continue }
            if ax(element, kAXRoleAttribute) as? String == kAXTextAreaRole { return element }
            pending.append(contentsOf: ax(element, kAXChildrenAttribute) as? [AXUIElement] ?? [])
        }
        return nil
    }

    func verifyTarget() throws {
        try verifyProcess()
        let front = NSWorkspace.shared.frontmostApplication?.processIdentifier ?? 0
        try require(front == arguments.pid && app.isActive,
                    "User focus left the standalone demo (frontPID=\(front), ownActive=\(app.isActive)); refusing further events")
        guard let window = axElement(ax(root, kAXFocusedWindowAttribute)), let focused = focusedArea() else {
            throw KeyProbeError.failed("The standalone editor is not the focused text area/key window")
        }
        var owner: pid_t = 0
        try require(AXUIElementGetPid(window, &owner) == .success && owner == arguments.pid,
                    "Focused window does not belong to the standalone demo")
        area = focused
    }

    func activate() throws {
        try verifyProcess()
        app.activate(options: [.activateAllWindows])
        // LaunchServices activation can be deferred for a just-created bundle.
        // Request frontmost on this verified application's AX object as well.
        _ = AXUIElementSetAttributeValue(root, kAXFrontmostAttribute as CFString, kCFBooleanTrue)
        if let window = axElement(ax(root, kAXFocusedWindowAttribute)) {
            _ = AXUIElementPerformAction(window, kAXRaiseAction as CFString)
            _ = AXUIElementSetAttributeValue(window, kAXMainAttribute as CFString, kCFBooleanTrue)
            _ = AXUIElementSetAttributeValue(window, kAXFocusedAttribute as CFString, kCFBooleanTrue)
        }
        _ = wait(3) { self.findArea() != nil }
        if let target = findArea() {
            let code = AXUIElementSetAttributeValue(target, kAXFocusedAttribute as CFString, kCFBooleanTrue)
            print("NATIVE_KEYS textAreaFound=true focusCode=\(code.rawValue)")
            if NSWorkspace.shared.frontmostApplication?.processIdentifier == arguments.pid,
               let bounds = axBounds(target) {
                try require(ax(target, kAXValueAttribute) as? String == "left TARGET right", "Refusing to focus text other than the fresh synthetic fixture")
                // A freshly activated NSWindow may still lack key-window focus.
                // Click only inside our verified frontmost synthetic text area;
                // then restore TARGET before typing any test characters.
                let point = CGPoint(x: bounds.minX + 24, y: bounds.minY + 24)
                guard let down = CGEvent(mouseEventSource: nil, mouseType: .leftMouseDown, mouseCursorPosition: point, mouseButton: .left),
                      let up = CGEvent(mouseEventSource: nil, mouseType: .leftMouseUp, mouseCursorPosition: point, mouseButton: .left) else {
                    throw KeyProbeError.failed("Cannot focus our native text area")
                }
                down.post(tap: .cghidEventTap)
                spin(0.05)
                up.post(tap: .cghidEventTap)
                spin(0.3)
                nativeFocusClick = true
                print("NATIVE_KEYS nativeFocusClick=true")
                var requested = CFRange(location: 5, length: 6)
                guard let range = AXValueCreate(.cfRange, &requested) else { throw KeyProbeError.failed("Cannot restore synthetic selection") }
                _ = AXUIElementSetAttributeValue(target, kAXSelectedTextRangeAttribute as CFString, range)
            }
        } else {
            print("NATIVE_KEYS textAreaFound=false")
        }
        let focused = wait(5) {
            NSWorkspace.shared.frontmostApplication?.processIdentifier == self.arguments.pid
                && self.app.isActive && self.focusedArea() != nil
                && axElement(ax(self.root, kAXFocusedWindowAttribute)) != nil
        }
        let role = axElement(ax(root, kAXFocusedUIElementAttribute)).flatMap { ax($0, kAXRoleAttribute) as? String } ?? "none"
        let ownAreaFocused = findArea().flatMap { ax($0, kAXFocusedAttribute) as? NSNumber }?.boolValue ?? false
        try require(focused, "Standalone demo did not become the active focused text area (frontPID=\(NSWorkspace.shared.frontmostApplication?.processIdentifier ?? 0), ownActive=\(app.isActive), role=\(role), textareaFocused=\(ownAreaFocused))")
        try verifyTarget()
        // The separate input-source driver reselects its source after observing
        // activation, because activating an app can change its input context.
        spin(arguments.startDelay)
        try verifyTarget()
        guard let area, let value = ax(area, kAXValueAttribute) as? String, let range = selectedRange(area) else {
            throw KeyProbeError.failed("Cannot verify the standalone synthetic source fixture")
        }
        try require(value == "left TARGET right" && range.location == 5 && range.length == 6,
                    "Refusing key input into anything other than the fresh selected synthetic TARGET fixture")
        print("NATIVE_KEYS ready pid=\(arguments.pid) mode=\(arguments.mode.rawValue) startDelay=\(arguments.startDelay) selectedFixture=true")
        fflush(stdout)
    }

    func key(_ code: CGKeyCode) throws {
        try verifyTarget()
        guard let down = CGEvent(keyboardEventSource: nil, virtualKey: code, keyDown: true),
              let up = CGEvent(keyboardEventSource: nil, virtualKey: code, keyDown: false) else {
            throw KeyProbeError.failed("Cannot construct a scan-code key event")
        }
        down.flags = []
        up.flags = []
        // No Unicode text is attached. The selected native input method owns
        // romaji/pinyin conversion, composition, candidates, and final commits.
        if arguments.hid {
            // IMK can require the WindowServer's native input path. This opt-in
            // route still verifies our exact frontmost text area before every
            // key, and attaches no Unicode payload or application command.
            down.post(tap: .cghidEventTap)
            up.post(tap: .cghidEventTap)
        } else {
            down.postToPid(arguments.pid)
            up.postToPid(arguments.pid)
        }
        posted += 1
        spin(0.15)
        try verifyTarget()
        print("NATIVE_KEYS posted=\(posted) scanCode=\(code)")
        fflush(stdout)
    }

    func candidateMetadata() throws {
        guard let output = arguments.candidateOutput else { return }
        try verifyTarget()
        let allowedNames: Set<String> = ["JapaneseIM-RomajiTyping", "JapaneseIM", "SCIM", "SCIMTyping", "ChineseIM"]
        var owners: [pid_t: String] = [arguments.pid: "standalone-demo"]
        let bundles = ["com.apple.JapaneseIM.RomajiTyping", "com.apple.JapaneseIM.KanaTyping", "com.apple.SCIM-Container", "com.apple.inputmethod.SCIM", "com.apple.inputmethod.TCIM"]
        for candidate in bundles.flatMap({ NSRunningApplication.runningApplications(withBundleIdentifier: $0) }) {
            guard let url = candidate.executableURL,
                  url.path.hasPrefix("/System/Library/Input Methods/"),
                  allowedNames.contains(url.lastPathComponent) else { continue }
            owners[candidate.processIdentifier] = url.lastPathComponent.contains("Japanese") ? "japanese-input-method" : "chinese-input-method"
        }
        var windows: [[String: Any]] = []
        // The API returns all window dictionaries. Only the PID is inspected
        // until it matches the narrow allowlist; no other-app titles, bounds,
        // pixels, or content are read or serialized.
        for (rank, info) in (CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]] ?? []).enumerated() {
            guard let number = info[kCGWindowOwnerPID as String] as? NSNumber,
                  let owner = owners[number.int32Value] else { continue }
            var item: [String: Any] = ["pid": number.int32Value, "owner": owner, "rank": rank]
            if let id = info[kCGWindowNumber as String] as? NSNumber { item["windowID"] = id.intValue }
            if let layer = info[kCGWindowLayer as String] as? NSNumber { item["layer"] = layer.intValue }
            if let bounds = info[kCGWindowBounds as String] as? [String: Any] {
                var clean: [String: Double] = [:]
                for key in ["X", "Y", "Width", "Height"] {
                    if let value = bounds[key] as? NSNumber { clean[key] = value.doubleValue }
                }
                item["bounds"] = clean
            }
            windows.append(item)
        }
        var payload: [String: Any] = ["kind": "native-conversion-candidate-metadata", "mode": arguments.mode.rawValue,
            "demoPID": arguments.pid, "postEventAllowed": CGPreflightPostEventAccess(), "AXTrusted": AXIsProcessTrusted(),
            "windowTitlesRead": false, "pixelsRead": false, "windows": windows]
        if let area {
            if let bounds = axBounds(area) { payload["editorBounds"] = rectJSON(bounds) }
            if let range = selectedRange(area) {
                payload["selectedUTF16Range"] = ["location": range.location, "length": range.length]
                if let bounds = rangeBounds(area, range) {
                    payload["selectedRangeBounds"] = rectJSON(bounds)
                    let nearbyPopup = windows.contains { item in
                        guard let layer = item["layer"] as? Int, layer > 0,
                              let box = item["bounds"] as? [String: Double],
                              let x = box["X"], let y = box["Y"],
                              let width = box["Width"], let height = box["Height"] else { return false }
                        return width > 0 && height > 0 && x <= bounds.midX && bounds.midX <= x + width
                            && abs(y - bounds.maxY) <= max(64, bounds.height * 2)
                    }
                    payload["candidateAnchorGeometry"] = [
                        "nearbyScopedPopup": nearbyPopup,
                        "identification": "Inference from controlled conversion timing and scoped popup bounds; no window titles or pixels read",
                    ]
                }
            }
        }
        let data = try JSONSerialization.data(withJSONObject: payload, options: [.prettyPrinted, .sortedKeys])
        try data.write(to: output, options: .atomic)
        print("NATIVE_KEYS candidateMetadata=WRITTEN windows=\(windows.count)")
        fflush(stdout)
    }

    // macOS Japanese input can consume Escape without ending its pending
    // session. Exercise the editor's explicit native cancel command as well.
    func explicitCancel() throws {
        try verifyTarget()
        var pending = ax(root, kAXWindowsAttribute) as? [AXUIElement] ?? []
        var visited: [AXUIElement] = []
        let deadline = Date().addingTimeInterval(3)
        while let element = pending.popLast(), visited.count < 512, Date() < deadline {
            if visited.contains(where: { CFEqual($0, element) }) { continue }
            visited.append(element)
            var owner: pid_t = 0
            guard AXUIElementGetPid(element, &owner) == .success, owner == arguments.pid else { continue }
            let label = (ax(element, kAXTitleAttribute) as? String) ?? (ax(element, kAXDescriptionAttribute) as? String)
            if ax(element, kAXRoleAttribute) as? String == kAXButtonRole && label == "Cancel IME" {
                try require(AXUIElementPerformAction(element, kAXPressAction as CFString) == .success, "Own editor cancel command failed")
                print("NATIVE_KEYS explicitEditorCancel=true escapeAloneQualified=false")
                return
            }
            pending.append(contentsOf: ax(element, kAXChildrenAttribute) as? [AXUIElement] ?? [])
        }
        throw KeyProbeError.failed("Cannot locate own editor's explicit cancel command")
    }

    func scrollPreedit() throws {
        try verifyTarget()
        guard let area, let bounds = axBounds(area),
              let move = CGEvent(mouseEventSource: nil, mouseType: .mouseMoved,
                  mouseCursorPosition: CGPoint(x: bounds.minX + 24, y: bounds.minY + 24), mouseButton: .left),
              let wheel = CGEvent(scrollWheelEvent2Source: nil, units: .pixel, wheelCount: 1,
                  wheel1: -64, wheel2: 0, wheel3: 0) else {
            throw KeyProbeError.failed("Cannot construct own-editor native scroll")
        }
        move.post(tap: .cghidEventTap)
        spin(0.05)
        try verifyTarget()
        // The wheel object was constructed before mouseMoved was delivered.
        // Its captured location must explicitly follow the post-zoom editor.
        wheel.location = CGPoint(x: bounds.minX + 24, y: bounds.minY + 24)
        wheel.setIntegerValueField(.scrollWheelEventIsContinuous, value: 1)
        wheel.post(tap: .cghidEventTap)
        spin(0.25)
        try verifyTarget()
        print("NATIVE_KEYS viewportScrollPosted=true")
    }

    func run() throws {
        try activate()
        switch arguments.mode {
        case .latin:
            for code: CGKeyCode in [35, 15, 31, 11, 14] { try key(code) } // probe
        case .japanese, .cancel, .focus:
            for (index, code) in ([45, 34, 4, 31, 45] as [CGKeyCode]).enumerated() {
                try key(code) // nihon
                if arguments.exerciseViewport && index == 2 { try scrollPreedit() }
            }
            if arguments.mode == .focus {
                try key(45) // Complete the final n: にほん.
                try verifyTarget()
                app.hide()
                spin(0.35)
                try verifyProcess()
                app.unhide()
                app.activate(options: [.activateAllWindows])
                _ = AXUIElementSetAttributeValue(root, kAXFrontmostAttribute as CFString, kCFBooleanTrue)
                _ = wait(3) { NSWorkspace.shared.frontmostApplication?.processIdentifier == self.arguments.pid }
                spin(0.25)
                try verifyTarget()
                print("NATIVE_KEYS ownAppFocusTransferred=true")
            } else if arguments.mode == .cancel {
                try key(53) // Cancel preedit directly, without a conversion stage.
                try key(53) // Japanese IME can first dismiss its conversion state.
                try key(53) // Escapes alone remain a separately observed platform behavior.
                try explicitCancel()
            } else {
                try key(49) // Space: native conversion/candidate presentation.
                spin(1)
                try candidateMetadata()
                try key(36)
                spin(0.35)
                try key(36) // Some native conversion UIs consume the first Return.
            }
        case .chinese:
            for (index, code) in ([6, 4, 31, 45, 5, 13, 14, 45] as [CGKeyCode]).enumerated() {
                try key(code) // zhongwen
                if arguments.exerciseViewport && index == 2 { try scrollPreedit() }
            }
            spin(1)
            try candidateMetadata()
            try key(49) // Pinyin's Space both selects the candidate and commits.
        }
        spin(0.25)
        if arguments.mode == .cancel {
            try verifyProcess()
            try require(NSWorkspace.shared.frontmostApplication?.processIdentifier == arguments.pid, "Focus left own editor after cancel")
        } else { try verifyTarget() }
        print("NATIVE_KEYS transport=POSTED mode=\(arguments.mode.rawValue) keys=\(posted) nativeResult=OBSERVER_REQUIRED")
    }
}

do {
    if let arguments = try Arguments.parse() { try NativeKeys(arguments).run() }
} catch {
    fputs("NATIVE_KEYS transport=FAIL \(error)\n", stderr)
    exit(1)
}
