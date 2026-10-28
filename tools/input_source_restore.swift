#!/usr/bin/env swift
// Opt-in restoration of the temporarily added Simplified Pinyin default mode.
// Uses normal Keyboard settings UI. A separate, explicitly authorized recovery
// removes only the exact recorded temporary mode entry through public
// CFPreferences APIs. It never resets a domain or changes TCC databases.
import AppKit
import ApplicationServices
import Carbon
import Foundation

func pause(_ seconds: Double) {
    let until = Date().addingTimeInterval(seconds)
    while Date() < until { RunLoop.current.run(until: min(until, Date().addingTimeInterval(0.03))) }
}
func ax(_ element: AXUIElement, _ key: String) -> CFTypeRef? {
    var result: CFTypeRef?
    guard AXUIElementCopyAttributeValue(element, key as CFString, &result) == .success else { return nil }
    return result
}
func role(_ element: AXUIElement) -> String { ax(element, kAXRoleAttribute) as? String ?? "" }
func children(_ element: AXUIElement) -> [AXUIElement] { ax(element, kAXChildrenAttribute) as? [AXUIElement] ?? [] }
func label(_ element: AXUIElement) -> String {
    (ax(element, kAXDescriptionAttribute) as? String).flatMap { $0.isEmpty ? nil : $0 }
        ?? (ax(element, kAXTitleAttribute) as? String).flatMap { $0.isEmpty ? nil : $0 }
        ?? (ax(element, kAXValueAttribute) as? String) ?? ""
}
func content(_ root: AXUIElement) -> [AXUIElement] {
    var queue = [root], output: [AXUIElement] = []
    var visited: [AXUIElement] = []
    let inputSheet = role(root) == kAXSheetRole
    while let element = queue.popLast(), visited.count < 1500 {
        guard !visited.contains(where: { CFEqual($0, element) }) else { continue }
        visited.append(element)
        // Settings sidebar and menu items are unrelated to input-source setup.
        guard ![kAXMenuBarRole, kAXToolbarRole].contains(role(element)),
              inputSheet || role(element) != kAXOutlineRole else { continue }
        output.append(element)
        queue.append(contentsOf: children(element).reversed())
    }
    return output
}
func source(_ id: String) -> TISInputSource? {
    (TISCreateInputSourceList(nil, true).takeRetainedValue() as! [TISInputSource]).first {
        guard let pointer = TISGetInputSourceProperty($0, kTISPropertyInputSourceID) else { return false }
        return Unmanaged<CFString>.fromOpaque(pointer).takeUnretainedValue() as String == id
    }
}
func enabled(_ source: TISInputSource) -> Bool {
    guard let pointer = TISGetInputSourceProperty(source, kTISPropertyInputSourceIsEnabled) else { return false }
    return CFBooleanGetValue(Unmanaged<CFBoolean>.fromOpaque(pointer).takeUnretainedValue())
}
func stop(_ message: String, parent: TISInputSource) -> Never {
    // Restore the operational parent even if the normal UI changes or times out.
    _ = TISDisableInputSource(parent)
    print("INPUT_SOURCE_UI failure", message)
    exit(1)
}

func restoreRecordedMetadata() -> Int32 {
    let domain = "com.apple.HIToolbox" as CFString
    let key = "AppleEnabledInputSources" as CFString
    CFPreferencesAppSynchronize(domain)
    let before = CFPreferencesCopyAppValue(key, domain) as? [NSDictionary] ?? []
    let extra: NSDictionary = ["Bundle ID": "com.apple.inputmethod.SCIM", "Input Mode": "com.apple.inputmethod.SCIM.ITABC", "InputSourceKind": "Input Mode"]
    let expected: NSArray = [
        ["InputSourceKind": "Keyboard Layout", "KeyboardLayout ID": 0, "KeyboardLayout Name": "U.S."],
        ["Bundle ID": "com.apple.CharacterPaletteIM", "InputSourceKind": "Non Keyboard Input Method"],
        ["Bundle ID": "com.apple.PressAndHold", "InputSourceKind": "Non Keyboard Input Method"],
    ]
    let retained = before.filter { !$0.isEqual(extra) }
    guard before.count == 4, before.filter({ $0.isEqual(extra) }).count == 1,
          (retained as NSArray).isEqual(expected),
          let parent = source("com.apple.inputmethod.SCIM"), !enabled(parent),
          let current = TISCopyCurrentKeyboardInputSource()?.takeRetainedValue(),
          let pointer = TISGetInputSourceProperty(current, kTISPropertyInputSourceID),
          Unmanaged<CFString>.fromOpaque(pointer).takeUnretainedValue() as String == "com.apple.keylayout.US" else {
        print("INPUT_SOURCE_METADATA abort: current configuration differs from exact recorded original plus temporary Pinyin entry")
        return 1
    }
    // Explicitly authorized recovery only: retain the three original objects,
    // remove exactly the one known test entry, and write no other domain/key.
    CFPreferencesSetAppValue(key, retained as NSArray, domain)
    let synced = CFPreferencesAppSynchronize(domain)
    let after = CFPreferencesCopyAppValue(key, domain) as? NSArray ?? []
    let restored = synced && after.isEqual(expected)
    let payload: [String: Any] = ["kind": "recorded-input-source-metadata-restoration", "restored": restored,
        "before": before, "after": after, "onlyKeyWritten": "AppleEnabledInputSources",
        "onlyDomainWritten": "com.apple.HIToolbox", "synchronized": synced,
        "requiresIndependentFreshProcessVerification": true]
    if let data = try? JSONSerialization.data(withJSONObject: payload, options: [.sortedKeys]) {
        print("INPUT_SOURCE_METADATA_RESTORED", String(decoding: data, as: UTF8.self))
    }
    return restored ? 0 : 1
}

if CommandLine.arguments.dropFirst().elementsEqual(["--help"]) {
    print("Usage: swift tools/input_source_restore.swift --restore-temporary-pinyin")
    print("       swift tools/input_source_restore.swift --restore-recorded-pinyin-metadata")
    print("The first uses normal Keyboard settings to remove the exact temporary Pinyin row. The second is explicitly authorized restoration of only the recorded extra default-mode preference; it aborts unless the full known baseline and current US source match.")
    exit(0)
}
if CommandLine.arguments.dropFirst().elementsEqual(["--restore-recorded-pinyin-metadata"]) {
    exit(restoreRecordedMetadata())
}

guard CommandLine.arguments.dropFirst().elementsEqual(["--restore-temporary-pinyin"]), AXIsProcessTrusted(),
      let parent = source("com.apple.inputmethod.SCIM"),
      let mode = source("com.apple.inputmethod.SCIM.ITABC"), enabled(mode) else {
    print("Expected authorized --restore-temporary-pinyin for a temporarily enabled Pinyin mode")
    exit(1)
}
guard enabled(parent) || TISEnableInputSource(parent) == noErr else { print("Cannot make temporary Pinyin row visible for removal"); exit(1) }
pause(0.4)
NSWorkspace.shared.open(URL(string: "x-apple.systempreferences:com.apple.Keyboard-Settings.extension")!)
pause(2)
guard let app = NSRunningApplication.runningApplications(withBundleIdentifier: "com.apple.systempreferences").first else { stop("Settings unavailable", parent: parent) }
let root = AXUIElementCreateApplication(app.processIdentifier)
AXUIElementSetMessagingTimeout(root, 0.5)
guard let window = (ax(root, kAXWindowsAttribute) as? [AXUIElement])?.first(where: { label($0).hasPrefix("Keyboard") }) else { stop("No Keyboard window", parent: parent) }
guard let inputLabel = content(window).first(where: { role($0) == kAXStaticTextRole && label($0) == "Input Sources" }) else { stop("No Input Sources label", parent: parent) }
guard let rawGroup = ax(inputLabel, kAXParentAttribute), CFGetTypeID(rawGroup) == AXUIElementGetTypeID() else { stop("No Input Sources parent", parent: parent) }
let group = unsafeBitCast(rawGroup, to: AXUIElement.self)
guard let edit = children(group).first(where: { role($0) == kAXButtonRole && label($0) == "Edit…" }) else { stop("No exact Edit button", parent: parent) }
print("INPUT_SOURCE_UI editTextInput", AXUIElementPerformAction(edit, kAXPressAction as CFString).rawValue)
pause(1)
guard let sheet = content(window).first(where: { role($0) == kAXSheetRole }),
      let row = content(sheet).first(where: { role($0) == kAXRowRole && content($0).contains(where: { label($0) == "Pinyin – Simplified" }) }),
      let remove = content(sheet).first(where: { role($0) == kAXButtonRole && label($0) == "Remove" }) else {
    stop("Exact Pinyin row or Remove missing", parent: parent)
}
guard AXUIElementSetAttributeValue(row, kAXSelectedAttribute as CFString, kCFBooleanTrue) == .success else {
    stop("Cannot select exact Pinyin row", parent: parent)
}
pause(0.2)
guard ax(row, kAXSelectedAttribute) as? Bool == true else { stop("Exact Pinyin row not selected", parent: parent) }
guard AXUIElementPerformAction(remove, kAXPressAction as CFString) == .success else { stop("Remove failed", parent: parent) }
print("INPUT_SOURCE_UI removedExactTemporaryPinyinRow=true")
pause(0.5)
if let done = content(sheet).first(where: { role($0) == kAXButtonRole && label($0) == "Done" }) {
    _ = AXUIElementPerformAction(done, kAXPressAction as CFString)
}
pause(0.5)
let domain = "com.apple.HIToolbox" as CFString
CFPreferencesAppSynchronize(domain)
let configuration = CFPreferencesCopyAppValue("AppleEnabledInputSources" as CFString, domain) as? [[String: Any]] ?? []
let expected: NSArray = [
    ["InputSourceKind": "Keyboard Layout", "KeyboardLayout ID": 0, "KeyboardLayout Name": "U.S."],
    ["Bundle ID": "com.apple.CharacterPaletteIM", "InputSourceKind": "Non Keyboard Input Method"],
    ["Bundle ID": "com.apple.PressAndHold", "InputSourceKind": "Non Keyboard Input Method"],
]
let parentRestored = source("com.apple.inputmethod.SCIM").map { !enabled($0) } ?? false
let modeRestored = source("com.apple.inputmethod.SCIM.ITABC").map { !enabled($0) } ?? false
let configurationRestored = (configuration as NSArray).isEqual(expected)
guard let current = TISCopyCurrentKeyboardInputSource()?.takeRetainedValue(),
      let currentPointer = TISGetInputSourceProperty(current, kTISPropertyInputSourceID) else { exit(1) }
let currentID = Unmanaged<CFString>.fromOpaque(currentPointer).takeUnretainedValue() as String
let restored = parentRestored && modeRestored && configurationRestored && currentID == "com.apple.keylayout.US"
let payload: [String: Any] = ["kind": "normal-input-source-ui-restoration", "restored": restored,
    "parentDisabled": parentRestored, "modeDisabled": modeRestored, "originalEnabledConfigurationRestored": configurationRestored,
    "currentInputSource": currentID, "enabledInputSources": configuration,
    "preferencesWrittenDirectly": false, "tccChanged": false]
if let data = try? JSONSerialization.data(withJSONObject: payload, options: [.sortedKeys]) {
    print("INPUT_SOURCE_UI_RESTORED", String(decoding: data, as: UTF8.self))
}
if let closeRaw = ax(window, kAXCloseButtonAttribute), CFGetTypeID(closeRaw) == AXUIElementGetTypeID() {
    _ = AXUIElementPerformAction(unsafeBitCast(closeRaw, to: AXUIElement.self), kAXPressAction as CFString)
}
exit(restored ? 0 : 1)
