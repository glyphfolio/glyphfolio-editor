#!/usr/bin/env swift
// Experimental authorized temporary setup through normal VoiceOver controls.
// The focused-Space restoration action succeeded in a separate native recovery;
// this integrated revision has compile-only validation and requires independent
// restoration verification. It is not a passed screen-reader qualification.
// No preference/TCC writes, global keys, or unrelated UI traversal.
import AppKit
import ApplicationServices
import Foundation

let projectDirectory = URL(fileURLWithPath: #filePath).standardizedFileURL.deletingLastPathComponent().deletingLastPathComponent()

enum SetupError: Error { case failed(String) }
func require(_ condition: Bool, _ message: String) throws { if !condition { throw SetupError.failed(message) } }
func spin(_ seconds: Double) {
    let until = Date().addingTimeInterval(seconds)
    while Date() < until { RunLoop.current.run(until: min(until, Date().addingTimeInterval(0.03))) }
}
func ax(_ element: AXUIElement, _ key: String) -> CFTypeRef? {
    AXUIElementSetMessagingTimeout(element, 0.5)
    var value: CFTypeRef?
    guard AXUIElementCopyAttributeValue(element, key as CFString, &value) == .success else { return nil }
    return value
}
func role(_ element: AXUIElement) -> String { ax(element, kAXRoleAttribute) as? String ?? "" }
func label(_ element: AXUIElement) -> String {
    (ax(element, kAXTitleAttribute) as? String).flatMap { $0.isEmpty ? nil : $0 }
        ?? (ax(element, kAXDescriptionAttribute) as? String).flatMap { $0.isEmpty ? nil : $0 }
        ?? (ax(element, kAXValueAttribute) as? String) ?? ""
}
func content(_ root: AXUIElement) -> [AXUIElement] {
    var queue = [root], result: [AXUIElement] = [], seen: [AXUIElement] = []
    while let element = queue.popLast(), seen.count < 1000 {
        guard !seen.contains(where: { CFEqual($0, element) }),
              ![kAXOutlineRole, kAXToolbarRole, kAXMenuBarRole].contains(role(element)) else { continue }
        seen.append(element)
        result.append(element)
        queue.append(contentsOf: (ax(element, kAXChildrenAttribute) as? [AXUIElement] ?? []).reversed())
    }
    return result
}
func capture(_ command: String, _ arguments: [String], timeout: Double = 5) throws -> String {
    let child = Process(), pipe = Pipe()
    child.executableURL = URL(fileURLWithPath: command)
    child.arguments = arguments
    child.standardOutput = pipe
    child.standardError = FileHandle.nullDevice
    try child.run()
    let deadline = Date().addingTimeInterval(timeout)
    while child.isRunning && Date() < deadline { spin(0.02) }
    if child.isRunning { child.terminate(); throw SetupError.failed("Bounded status helper timed out") }
    return String(decoding: pipe.fileHandleForReading.readDataToEndOfFile(), as: UTF8.self)
}
func freshStatus() throws -> String {
    let source = "import AppKit; print(NSWorkspace.shared.isVoiceOverEnabled ? \"true\" : \"false\")"
    return try capture("/usr/bin/swift", ["-module-cache-path", projectDirectory.appendingPathComponent("target/swift-module-cache").path, "-e", source]).trimmingCharacters(in: .whitespacesAndNewlines)
}
func enginePIDs() throws -> [Int] {
    let result = try capture("/usr/bin/pgrep", ["-f", "^/System/Library/CoreServices/VoiceOver.app/Contents/MacOS/VoiceOver( launchd.*)?$"])
    return result.split(whereSeparator: \.isWhitespace).compactMap { Int($0) }
}

func close(_ window: AXUIElement) {
    if let raw = ax(window, kAXCloseButtonAttribute), CFGetTypeID(raw) == AXUIElementGetTypeID() {
        _ = AXUIElementPerformAction(unsafeBitCast(raw, to: AXUIElement.self), kAXPressAction as CFString)
    }
}
func handleWelcome() {
    for app in NSRunningApplication.runningApplications(withBundleIdentifier: "com.apple.VoiceOverQuickstart") {
        let root = AXUIElementCreateApplication(app.processIdentifier)
        AXUIElementSetMessagingTimeout(root, 0.5)
        for window in ax(root, kAXWindowsAttribute) as? [AXUIElement] ?? [] {
            let nodes = content(window)
            guard nodes.contains(where: { role($0) == kAXStaticTextRole && label($0) == "Welcome to VoiceOver" }),
                  let use = nodes.first(where: { role($0) == kAXButtonRole && label($0) == "Use VoiceOver" }) else { continue }
            // Do not press or modify the separate reminder checkbox.
            let code = AXUIElementPerformAction(use, kAXPressAction as CFString)
            print("VOICEOVER_SETUP welcomeUsePressCode=\(code.rawValue)")
        }
    }
}
func pressFocusedCheckbox(_ checkbox: AXUIElement, app: NSRunningApplication, root: AXUIElement) throws {
    try require(CGPreflightPostEventAccess(), "Native event-posting permission is unavailable")
    var owner: pid_t = 0
    try require(AXUIElementGetPid(checkbox, &owner) == .success && owner == app.processIdentifier
        && role(checkbox) == kAXCheckBoxRole && label(checkbox) == "Allow VoiceOver to be controlled with AppleScript",
        "Refusing an unexpected checkbox or process")
    app.activate(options: [.activateAllWindows])
    _ = AXUIElementSetAttributeValue(root, kAXFrontmostAttribute as CFString, kCFBooleanTrue)
    spin(0.2)
    if let window = (ax(root, kAXWindowsAttribute) as? [AXUIElement] ?? []).first(where: { label($0) == "VoiceOver Utility" }) {
        _ = AXUIElementSetAttributeValue(window, kAXMainAttribute as CFString, kCFBooleanTrue)
        _ = AXUIElementSetAttributeValue(window, kAXFocusedAttribute as CFString, kCFBooleanTrue)
    }
    try require(AXUIElementSetAttributeValue(checkbox, kAXFocusedAttribute as CFString, kCFBooleanTrue) == .success,
                "Cannot focus the exact script-control checkbox")
    guard let focusedRaw = ax(root, kAXFocusedUIElementAttribute), CFGetTypeID(focusedRaw) == AXUIElementGetTypeID() else {
        throw SetupError.failed("Native checkbox focus was not acknowledged")
    }
    try require(CFEqual(unsafeBitCast(focusedRaw, to: AXUIElement.self), checkbox)
        && NSWorkspace.shared.frontmostApplication?.processIdentifier == app.processIdentifier,
        "Utility focus changed; refusing keyboard input")
    guard let down = CGEvent(keyboardEventSource: nil, virtualKey: 49, keyDown: true),
          let up = CGEvent(keyboardEventSource: nil, virtualKey: 49, keyDown: false) else {
        throw SetupError.failed("Cannot construct native Space for the focused checkbox")
    }
    down.flags = []; up.flags = []
    down.postToPid(app.processIdentifier)
    up.postToPid(app.processIdentifier)
    spin(0.2)
    print("VOICEOVER_SETUP exactFocusedCheckboxSpace=true")
}

func freshScriptControl() throws -> Bool? {
    let source = #"import AppKit; import ApplicationServices; guard let app=NSRunningApplication.runningApplications(withBundleIdentifier:"com.apple.VoiceOverUtility").first else{print("unknown");exit(0)};let root=AXUIElementCreateApplication(app.processIdentifier);func v(_ e:AXUIElement,_ k:String)->CFTypeRef?{AXUIElementSetMessagingTimeout(e,0.5);var r:CFTypeRef?;AXUIElementCopyAttributeValue(e,k as CFString,&r);return r};func walk(_ e:AXUIElement,_ d:Int)->Bool?{if d>12{return nil};if v(e,kAXRoleAttribute) as? String==kAXCheckBoxRole && v(e,kAXTitleAttribute) as? String=="Allow VoiceOver to be controlled with AppleScript"{return (v(e,kAXValueAttribute) as? NSNumber)?.boolValue};for c in v(e,kAXChildrenAttribute) as? [AXUIElement] ?? []{if let result=walk(c,d+1){return result}};return nil};for w in v(root,kAXWindowsAttribute) as? [AXUIElement] ?? []{if let result=walk(w,0){print(result ? "true":"false");exit(0)}};print("unknown")"#
    let result = try capture("/usr/bin/swift", ["-module-cache-path", projectDirectory.appendingPathComponent("target/swift-module-cache").path, "-e", source])
        .trimmingCharacters(in: .whitespacesAndNewlines)
    return result == "true" ? true : result == "false" ? false : nil
}
func setVoiceOver(_ desired: Bool) throws -> Bool {
    try require(AXIsProcessTrusted(), "Accessibility access is unavailable")
    let url = URL(string: "x-apple.systempreferences:com.apple.preference.universalaccess?Seeing_VoiceOver")!
    NSWorkspace.shared.open(url)
    spin(1)
    guard let app = NSRunningApplication.runningApplications(withBundleIdentifier: "com.apple.systempreferences").first else { throw SetupError.failed("Normal settings application is unavailable") }
    let root = AXUIElementCreateApplication(app.processIdentifier)
    AXUIElementSetMessagingTimeout(root, 0.5)
    var window: AXUIElement?
    let deadline = Date().addingTimeInterval(5)
    while Date() < deadline && window == nil {
        window = (ax(root, kAXWindowsAttribute) as? [AXUIElement] ?? []).first { label($0) == "VoiceOver" }
        spin(0.1)
    }
    guard let window else { throw SetupError.failed("Exact VoiceOver settings window is unavailable") }
    var checks = content(window).filter { role($0) == kAXCheckBoxRole }
    let contentDeadline = Date().addingTimeInterval(6)
    while checks.count != 1 && Date() < contentDeadline {
        spin(0.1)
        checks = content(window).filter { role($0) == kAXCheckBoxRole }
    }
    let named = content(window).filter { role($0) == kAXStaticTextRole && label($0) == "VoiceOver" }.count
    try require(checks.count == 1 && named > 0, "VoiceOver checkbox is ambiguous (checkboxes=\(checks.count), exactLabels=\(named))")
    let checkbox = checks[0]
    guard let current = ax(checkbox, kAXValueAttribute) as? NSNumber else { throw SetupError.failed("VoiceOver checkbox value is unavailable") }
    if current.boolValue != desired {
        let result = AXUIElementPerformAction(checkbox, kAXPressAction as CFString)
        print("VOICEOVER_SETUP checkboxPressCode=\(result.rawValue)")
    }
    spin(0.5)
    if desired { handleWelcome() }
    var after = try freshStatus(), pids = try enginePIDs()
    let serviceDeadline = Date().addingTimeInterval(8)
    while Date() < serviceDeadline && (desired ? after != "true" || pids.isEmpty : after != "false" || !pids.isEmpty) {
        spin(0.2)
        if desired && pids.isEmpty { handleWelcome() }
        after = try freshStatus()
        pids = try enginePIDs()
    }
    let payload: [String: Any] = ["requestedEnabled": desired, "freshEnabledFlag": after == "true", "enginePIDs": pids,
        "checkboxValue": ax(checkbox, kAXValueAttribute) as? NSNumber ?? -1,
        "setupAccepted": desired ? after == "true" && !pids.isEmpty : after == "false" && pids.isEmpty,
        "preferencesWrittenDirectly": false, "tccChanged": false]
    let data = try JSONSerialization.data(withJSONObject: payload, options: [.sortedKeys])
    print("VOICEOVER_SETUP_RESULT", String(decoding: data, as: UTF8.self))
    close(window)
    return payload["setupAccepted"] as? Bool == true
}

func scriptControl(set desired: Bool? = nil) throws -> Bool {
    guard let url = NSWorkspace.shared.urlForApplication(withBundleIdentifier: "com.apple.VoiceOverUtility") else { throw SetupError.failed("VoiceOver Utility is unavailable") }
    NSWorkspace.shared.open(url)
    var running = NSRunningApplication.runningApplications(withBundleIdentifier: "com.apple.VoiceOverUtility").first
    let launchDeadline = Date().addingTimeInterval(5)
    while running == nil && Date() < launchDeadline {
        spin(0.2)
        running = NSRunningApplication.runningApplications(withBundleIdentifier: "com.apple.VoiceOverUtility").first
        if running == nil { NSWorkspace.shared.open(url) }
    }
    guard let app = running else { throw SetupError.failed("VoiceOver Utility did not open") }
    let root = AXUIElementCreateApplication(app.processIdentifier)
    AXUIElementSetMessagingTimeout(root, 0.5)
    var window: AXUIElement?, checkbox: AXUIElement?
    let deadline = Date().addingTimeInterval(5)
    while Date() < deadline && checkbox == nil {
        window = (ax(root, kAXWindowsAttribute) as? [AXUIElement] ?? []).first { label($0) == "VoiceOver Utility" }
        if let window {
            let nodes = content(window)
            if let general = nodes.first(where: { role($0) == kAXRowRole && content($0).contains { role($0) == kAXStaticTextRole && label($0) == "General" } }),
               (ax(general, kAXSelectedAttribute) as? NSNumber)?.boolValue != true {
                _ = AXUIElementSetAttributeValue(general, kAXSelectedAttribute as CFString, kCFBooleanTrue)
                spin(0.3)
            }
            checkbox = content(window).first { role($0) == kAXCheckBoxRole && label($0) == "Allow VoiceOver to be controlled with AppleScript" }
        }
        spin(0.1)
    }
    guard let window, let checkbox, let value = ax(checkbox, kAXValueAttribute) as? NSNumber else { throw SetupError.failed("Exact General AppleScript-control checkbox is unavailable") }
    let before = value.boolValue
    if let desired, before != desired {
        print("VOICEOVER_SETUP scriptControlEnabled=\((ax(checkbox, kAXEnabledAttribute) as? NSNumber)?.boolValue ?? false)")
        try pressFocusedCheckbox(checkbox, app: app, root: root)
        var acknowledged = try freshScriptControl() == desired
        let settingDeadline = Date().addingTimeInterval(12)
        while !acknowledged && Date() < settingDeadline {
            spin(0.2)
            acknowledged = try freshScriptControl() == desired
        }
        if !acknowledged {
            for candidate in ax(root, kAXWindowsAttribute) as? [AXUIElement] ?? [] {
                print("VOICEOVER_SETUP utilityWindow", label(candidate), ax(candidate, kAXSubroleAttribute) as? String ?? "", ax(candidate, kAXModalAttribute) as? NSNumber ?? false)
                for item in content(candidate) where role(item) == kAXButtonRole {
                    print("VOICEOVER_SETUP utilityButton", label(item))
                }
                for sheet in content(candidate).filter({ role($0) == kAXSheetRole }) {
                    for item in content(sheet) where [kAXStaticTextRole, kAXButtonRole].contains(role(item)) {
                        print("VOICEOVER_SETUP confirmation", role(item), label(item))
                    }
                }
            }
        }
        guard acknowledged else { throw SetupError.failed("AppleScript-control checkbox did not acknowledge the requested state") }
    }
    // Keep the read-before-write window alive: this utility may terminate when
    // its last window closes, which races an immediate second launch/set call.
    if desired != nil { close(window) }
    return before
}

struct Journal: Codable {
    var active: Bool
    var initialEnabled: Bool
    var initialScriptControl: Bool?
    var utilityOriginallyRunning: Bool
    var restored: Bool
}
let journalURL = projectDirectory.appendingPathComponent("target/voiceover-setup-state.json")
func save(_ journal: Journal) throws {
    try JSONEncoder().encode(journal).write(to: journalURL, options: .atomic)
}
func load() -> Journal? {
    guard let data = try? Data(contentsOf: journalURL) else { return nil }
    return try? JSONDecoder().decode(Journal.self, from: data)
}
func restore(_ saved: inout Journal) throws -> Bool {
    let restoredService = try setVoiceOver(saved.initialEnabled)
    if let value = saved.initialScriptControl { _ = try scriptControl(set: value) }
    if !saved.utilityOriginallyRunning {
        NSRunningApplication.runningApplications(withBundleIdentifier: "com.apple.VoiceOverUtility").forEach { $0.terminate() }
    }
    saved.restored = restoredService
    saved.active = !restoredService
    try save(saved)
    return restoredService
}
func run() throws -> Int32 {
    let args = Array(CommandLine.arguments.dropFirst())
    if args == ["--help"] {
        print("Usage: swift tools/voiceover_setup.swift --enable [--with-script-control] | --disable | --status")
        print("EXPERIMENTAL: integrated focused-Space revision is compile-checked only; verify restoration independently. VoiceOver reader qualification remains incomplete.")
        print("Normal VoiceOver/General controls only. A project-local journal records the prior script-control setting; --disable restores it. Exact welcome handling leaves the reminder checkbox untouched. Fresh processes verify the enabled flag and actual engine PID.")
        return 0
    }
    let initial = try freshStatus(), pids = try enginePIDs()
    print("VOICEOVER_SETUP before=\(initial) enginePIDs=\(pids)")
    if args == ["--status"] { return 0 }
    if args == ["--disable"] {
        if var saved = load(), saved.active { return try restore(&saved) ? 0 : 1 }
        return try setVoiceOver(false) ? 0 : 1
    }
    guard args == ["--enable"] || args == ["--enable", "--with-script-control"] else { throw SetupError.failed("Expected --enable [--with-script-control], --disable, or --status") }
    if let existing = load(), existing.active { throw SetupError.failed("An owned setup journal is still active; run --disable first") }
    try require(initial == "false" && pids.isEmpty, "Refusing to take ownership of VoiceOver that was already enabled")
    var saved = Journal(active: true, initialEnabled: false, initialScriptControl: nil,
        utilityOriginallyRunning: !NSRunningApplication.runningApplications(withBundleIdentifier: "com.apple.VoiceOverUtility").isEmpty,
        restored: false)
    try save(saved)
    do {
        if args.contains("--with-script-control") {
            saved.initialScriptControl = try scriptControl()
            try save(saved)
            _ = try scriptControl(set: true)
        }
        if try setVoiceOver(true) { return 0 }
        _ = try restore(&saved)
        return 1
    } catch {
        _ = try? restore(&saved)
        throw error
    }
}
do { exit(try run()) } catch { print("VOICEOVER_SETUP_ERROR", error); exit(1) }
