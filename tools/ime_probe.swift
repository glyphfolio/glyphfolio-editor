#!/usr/bin/env swift
// Own-demo input-source driver. Selects an installed source and restores the
// original selected/enabled state after exit, errors, or handled signals.
// --enable-temporarily requires explicit authorization for temporary native test
// setup. It changes no accessibility/TCC setting. The optional guarded key
// driver supports PID events or explicitly requested native HID transport.
import Carbon
import AppKit
import Dispatch
import Foundation
import Darwin

enum DriverError: Error, CustomStringConvertible {
    case invalidArguments(String)
    case missingCurrentSource
    case unavailableSource(String)
    case sourceSelectionFailed(OSStatus)
    case sourceEnableFailed(OSStatus)
    case sourceDidNotChange(String)

    var description: String {
        switch self {
        case .invalidArguments(let message): return message
        case .missingCurrentSource: return "Cannot read the current keyboard input source"
        case .unavailableSource(let id): return "Source is unavailable, unselectable, or not enabled without --enable-temporarily: \(id)"
        case .sourceSelectionFailed(let status): return "TISSelectInputSource failed: \(status)"
        case .sourceEnableFailed(let status): return "TISEnableInputSource failed: \(status)"
        case .sourceDidNotChange(let id): return "Selected source did not become current: \(id)"
        }
    }
}

func textProperty(_ source: TISInputSource, _ key: CFString) -> String? {
    guard let pointer = TISGetInputSourceProperty(source, key) else { return nil }
    return Unmanaged<CFString>.fromOpaque(pointer).takeUnretainedValue() as String
}

func boolProperty(_ source: TISInputSource, _ key: CFString) -> Bool {
    guard let pointer = TISGetInputSourceProperty(source, key) else { return false }
    return CFBooleanGetValue(Unmanaged<CFBoolean>.fromOpaque(pointer).takeUnretainedValue())
}

func sourceID(_ source: TISInputSource) -> String {
    textProperty(source, kTISPropertyInputSourceID) ?? "<unknown>"
}

func sources(includeInstalled: Bool = true) -> [TISInputSource] {
    let list = TISCreateInputSourceList(nil, includeInstalled).takeRetainedValue()
    return (0..<CFArrayGetCount(list)).compactMap { index in
        guard let pointer = CFArrayGetValueAtIndex(list, index) else { return nil }
        return Unmanaged<TISInputSource>.fromOpaque(pointer).takeUnretainedValue()
    }
}

func currentSource() -> TISInputSource? {
    TISCopyCurrentKeyboardInputSource()?.takeRetainedValue()
}

func enabledConfiguration() -> NSArray {
    let domain = "com.apple.HIToolbox" as CFString
    CFPreferencesAppSynchronize(domain)
    return CFPreferencesCopyAppValue("AppleEnabledInputSources" as CFString, domain) as? NSArray ?? []
}

func emit(_ event: String, _ fields: [String: Any] = [:]) {
    var payload = fields
    payload["event"] = event
    guard let data = try? JSONSerialization.data(withJSONObject: payload, options: [.sortedKeys]) else { return }
    FileHandle.standardOutput.write(Data("INPUT_SOURCE_STATE ".utf8) + data + Data("\n".utf8))
}

func freshSourceState() -> [String: Any]? {
    let process = Process()
    let output = Pipe()
    process.executableURL = URL(fileURLWithPath: "/usr/bin/swift")
    let sourcePath = URL(fileURLWithPath: #filePath).standardizedFileURL
    let cache = sourcePath.deletingLastPathComponent().deletingLastPathComponent()
        .appendingPathComponent("target/swift-module-cache").path
    process.arguments = ["-module-cache-path", cache, sourcePath.path, "--state-json"]
    process.standardOutput = output
    process.standardError = FileHandle.standardError
    do { try process.run() } catch { return nil }
    let deadline = Date().addingTimeInterval(8)
    while process.isRunning && Date() < deadline { Thread.sleep(forTimeInterval: 0.025) }
    if process.isRunning { process.terminate() }
    process.waitUntilExit()
    guard process.terminationStatus == 0 else { return nil }
    return (try? JSONSerialization.jsonObject(with: output.fileHandleForReading.readDataToEndOfFile())) as? [String: Any]
}

final class SourceRestoration {
    let prior: TISInputSource
    let priorID: String
    let setupBundleID: String?
    let originallyEnabled: Set<String>
    let originallyOperational: Set<String>
    let originalConfiguration: NSArray
    let includesJapaneseCompanion: Bool
    let restoreIntroducedConfiguration: Bool
    var enabledBySetup: [TISInputSource] = []
    var restored = false

    init(_ source: TISInputSource, target: TISInputSource, restoreIntroducedConfiguration: Bool) {
        prior = source
        priorID = sourceID(source)
        setupBundleID = textProperty(target, kTISPropertyBundleID)
        originallyEnabled = Set(sources().filter { boolProperty($0, kTISPropertyInputSourceIsEnabled) }.map(sourceID))
        originallyOperational = Set(sources(includeInstalled: false).map(sourceID))
        originalConfiguration = enabledConfiguration()
        includesJapaneseCompanion = sourceID(target).contains("Kotoeri")
        self.restoreIntroducedConfiguration = restoreIntroducedConfiguration
    }

    func recordEnabledBySetup() {
        // Enabling an input mode can also enable its containing input method.
        // Journal only newly enabled entries in the explicitly requested family.
        enabledBySetup = sources().filter {
            boolProperty($0, kTISPropertyInputSourceIsEnabled)
                && !originallyEnabled.contains(sourceID($0))
                && (textProperty($0, kTISPropertyBundleID) == setupBundleID
                    || (includesJapaneseCompanion && sourceID($0) == "com.apple.50onPaletteIM"))
        }.sorted { sourceID($0).count > sourceID($1).count }
        emit("temporaryEnabled", ["ids": enabledBySetup.map(sourceID)])
    }

    func restore() {
        recordEnabledBySetup()
        let status = TISSelectInputSource(prior)
        let until = Date().addingTimeInterval(2)
        while Date() < until && currentSource().map(sourceID) != priorID {
            Thread.sleep(forTimeInterval: 0.05)
        }
        let actual = currentSource().map(sourceID) ?? "<missing>"
        var disabledCleanly = true
        for source in enabledBySetup {
            let disabled = TISDisableInputSource(source)
            let enabled = boolProperty(source, kTISPropertyInputSourceIsEnabled)
            // Final flags are checked after preference reconciliation; a mode
            // can briefly retain its default enabled flag during TIS cleanup.
            disabledCleanly = disabledCleanly && disabled == noErr
            emit("temporaryDisabled", ["id": sourceID(source), "status": disabled, "stillEnabled": enabled])
        }
        // macOS may retain an introduced default-mode preference even after
        // TISDisable and normal UI removal. Restore just this journaled value,
        // only if its sole differences are newly introduced family entries.
        // Any unrelated concurrent change aborts this cleanup without a write.
        let afterDisable = enabledConfiguration()
        if restoreIntroducedConfiguration && !afterDisable.isEqual(originalConfiguration) {
            let filtered = afterDisable.filter { raw in
                guard let entry = raw as? NSDictionary,
                      !originalConfiguration.contains(entry) else { return true }
                let bundle = entry["Bundle ID"] as? String
                return bundle != setupBundleID
                    && !(includesJapaneseCompanion && bundle == "com.apple.50onPaletteIM")
            } as NSArray
            if filtered.isEqual(originalConfiguration) {
                let domain = "com.apple.HIToolbox" as CFString
                CFPreferencesSetAppValue("AppleEnabledInputSources" as CFString, originalConfiguration, domain)
                emit("introducedConfigurationRestored", ["synchronized": CFPreferencesAppSynchronize(domain), "scope": "AppleEnabledInputSources only"])
            } else {
                emit("configurationCleanupSkipped", ["reason": "Unrelated configuration difference; original value was not written"])
            }
        }
        let cachedFlagsRestored = Set(sources().filter { boolProperty($0, kTISPropertyInputSourceIsEnabled) }.map(sourceID)) == originallyEnabled
        let configurationDeadline = Date().addingTimeInterval(2)
        while Date() < configurationDeadline && !enabledConfiguration().isEqual(originalConfiguration) {
            Thread.sleep(forTimeInterval: 0.05)
        }
        let fresh = freshSourceState()
        let operationalRestored = fresh.flatMap { $0["operationalIDs"] as? [String] }.map { Set($0) == originallyOperational } ?? false
        let flagsRestored = fresh.flatMap { $0["enabledIDs"] as? [String] }.map { Set($0) == originallyEnabled } ?? false
        let configurationRestored = (fresh?["configuration"] as? NSArray)?.isEqual(originalConfiguration) ?? false
        let freshSelectedRestored = fresh?["selectedID"] as? String == priorID
        // Source enumeration may lazily discover latent input modes. Persisted
        // enabled configuration plus per-source flags are the restoration contract;
        // retain enumeration equality as diagnostic data rather than hiding it.
        restored = status == noErr && actual == priorID && freshSelectedRestored && disabledCleanly && flagsRestored && configurationRestored
        emit("restored", ["expected": priorID, "actual": actual, "status": status, "restored": restored, "enabledFlagsRestored": flagsRestored, "cachedEnabledFlagsRestored": cachedFlagsRestored, "independentProcessVerified": fresh != nil, "enabledConfigurationRestored": configurationRestored, "operationalSourcesRestored": operationalRestored, "temporarilyEnabledIDs": enabledBySetup.map(sourceID)])
    }
}

final class ChildRuntime: @unchecked Sendable {
    let child = Process()
    private let lock = NSLock()
    private var timeoutFlag = false
    private var signalNumber: Int32?

    func stop(timedOut: Bool = false, signal: Int32? = nil) {
        lock.lock()
        timeoutFlag = timeoutFlag || timedOut
        if let signal { signalNumber = signal }
        lock.unlock()
        if child.isRunning { child.terminate() }
        DispatchQueue.global().asyncAfter(deadline: .now() + 2) { [self] in
            // Exact owned child only. Do not target other applications.
            if child.isRunning { _ = Darwin.kill(child.processIdentifier, SIGKILL) }
        }
    }

    func outcome() -> (Bool, Int32?) {
        lock.lock()
        defer { lock.unlock() }
        return (timeoutFlag, signalNumber)
    }
}

func run() -> Int32 {
    var args = Array(CommandLine.arguments.dropFirst())
    if args == ["--state-json"] {
        let payload: [String: Any] = [
            "selectedID": currentSource().map(sourceID) ?? "<missing>",
            "enabledIDs": sources().filter { boolProperty($0, kTISPropertyInputSourceIsEnabled) }.map(sourceID),
            "operationalIDs": sources(includeInstalled: false).map(sourceID),
            "configuration": enabledConfiguration(),
        ]
        guard let data = try? JSONSerialization.data(withJSONObject: payload, options: [.sortedKeys]) else { return 1 }
        FileHandle.standardOutput.write(data)
        return 0
    }
    if args == ["--list"] || args == ["--list-installed"] {
        let installed = args == ["--list-installed"]
        emit("current", ["id": currentSource().map(sourceID) ?? "<missing>"])
        for source in sources() where installed || boolProperty(source, kTISPropertyInputSourceIsEnabled) {
            emit(installed ? "installed" : "enabled", [
                "id": sourceID(source),
                "name": textProperty(source, kTISPropertyLocalizedName) ?? "",
                "enabled": boolProperty(source, kTISPropertyInputSourceIsEnabled),
                "selectable": boolProperty(source, kTISPropertyInputSourceIsSelectCapable),
            ])
        }
        return 0
    }
    if args.isEmpty || args == ["--help"] {
        print("Usage: swift tools/ime_probe.swift --list")
        print("       swift tools/ime_probe.swift --list-installed")
        print("       swift tools/ime_probe.swift --source INSTALLED_ID --binary /absolute/glyphfolio-editor-demo [--enable-temporarily] [--key-driver ABSOLUTE_PATH --key-mode japanese|chinese|cancel|latin|focus --key-transport pid|hid --key-exercise-viewport yes|no] [--candidate-output ABSOLUTE_JSON] [--timeout 45] -- [demo arguments]")
        print("The exact demo child is bounded by timeout. Prior selected source is restored, and only source-family entries enabled by the opt-in setup are disabled afterward. No accessibility setting is changed.")
        return args.isEmpty ? 1 : 0
    }

    var requested: String?
    var binary: String?
    var timeout: Double = 30
    var keyDriver: String?
    var keyMode = "japanese"
    var candidateOutput: String?
    var keyTransport = "pid"
    var exerciseViewport = "no"
    var allowTemporaryEnable = false
    var childArgs: [String] = []
    do {
        while !args.isEmpty {
            let arg = args.removeFirst()
            if arg == "--" { childArgs = args; break }
            if arg == "--enable-temporarily" { allowTemporaryEnable = true; continue }
            guard !args.isEmpty else { throw DriverError.invalidArguments("Missing value for \(arg)") }
            let value = args.removeFirst()
            switch arg {
            case "--source": requested = value
            case "--binary": binary = value
            case "--key-driver": keyDriver = value
            case "--key-mode": keyMode = value
            case "--candidate-output": candidateOutput = value
            case "--key-transport":
                guard value == "pid" || value == "hid" else { throw DriverError.invalidArguments("Expected pid or hid key transport") }
                keyTransport = value
            case "--key-exercise-viewport":
                guard value == "yes" || value == "no" else { throw DriverError.invalidArguments("Expected yes or no viewport exercise") }
                exerciseViewport = value
            case "--timeout":
                guard let parsed = Double(value), parsed >= 1, parsed <= 45 else {
                    throw DriverError.invalidArguments("Timeout must be 1–45 seconds")
                }
                timeout = parsed
            default: throw DriverError.invalidArguments("Unknown driver option: \(arg)")
            }
        }
        guard let requested, let binary,
              URL(fileURLWithPath: binary).lastPathComponent == "glyphfolio-editor-demo",
              binary.hasPrefix("/"), FileManager.default.isExecutableFile(atPath: binary) else {
            throw DriverError.invalidArguments("An installed source and absolute executable glyphfolio-editor-demo path are required")
        }
        guard let target = sources().first(where: {
            sourceID($0) == requested
                && (boolProperty($0, kTISPropertyInputSourceIsEnabled) || allowTemporaryEnable)
                && boolProperty($0, kTISPropertyInputSourceIsSelectCapable)
        }) else { throw DriverError.unavailableSource(requested) }
        guard let prior = currentSource() else { throw DriverError.missingCurrentSource }
        let restore = SourceRestoration(prior, target: target, restoreIntroducedConfiguration: allowTemporaryEnable)
        let runtime = ChildRuntime()
        let keys = Process()
        var result: Int32 = 1
        do {
            runtime.child.executableURL = URL(fileURLWithPath: binary)
            runtime.child.arguments = childArgs
            runtime.child.standardOutput = FileHandle.standardOutput
            runtime.child.standardError = FileHandle.standardError
            var environment = ProcessInfo.processInfo.environment
            environment["GLYPHFOLIO_PROBE_INPUT_SOURCE"] = requested
            runtime.child.environment = environment

            let oldINT = Darwin.signal(SIGINT, SIG_IGN)
            let oldTERM = Darwin.signal(SIGTERM, SIG_IGN)
            let interrupt = DispatchSource.makeSignalSource(signal: SIGINT, queue: .global())
            let terminate = DispatchSource.makeSignalSource(signal: SIGTERM, queue: .global())
            interrupt.setEventHandler { runtime.stop(signal: SIGINT) }
            terminate.setEventHandler { runtime.stop(signal: SIGTERM) }
            interrupt.resume()
            terminate.resume()
            defer {
                interrupt.cancel()
                terminate.cancel()
                Darwin.signal(SIGINT, oldINT)
                Darwin.signal(SIGTERM, oldTERM)
            }
            // Register after handler cleanup so restoration executes first,
            // while interrupts are still handled during its bounded checks.
            defer { restore.restore() }
            defer {
                if keys.isRunning { keys.terminate(); keys.waitUntilExit() }
                if runtime.child.isRunning { runtime.stop(signal: SIGTERM); runtime.child.waitUntilExit() }
            }
            let wasEnabled = boolProperty(target, kTISPropertyInputSourceIsEnabled)
            emit("before", ["id": restore.priorID, "requested": requested, "targetWasEnabled": wasEnabled])
            // A mode can have a latent enabled flag while its containing input
            // method is disabled. Enable only explicitly authorized ancestors.
            let parents = sources().filter {
                requested.hasPrefix(sourceID($0) + ".")
                    && !boolProperty($0, kTISPropertyInputSourceIsSelectCapable)
                    && !boolProperty($0, kTISPropertyInputSourceIsEnabled)
            }.sorted { sourceID($0).count < sourceID($1).count }
            for parent in parents {
                guard allowTemporaryEnable else { throw DriverError.unavailableSource(sourceID(parent)) }
                let enabled = TISEnableInputSource(parent)
                restore.recordEnabledBySetup()
                emit("parentEnabled", ["id": sourceID(parent), "status": enabled])
                guard enabled == noErr else { throw DriverError.sourceEnableFailed(enabled) }
            }
            if !wasEnabled {
                let enabled = TISEnableInputSource(target)
                restore.recordEnabledBySetup()
                guard enabled == noErr else { throw DriverError.sourceEnableFailed(enabled) }
            }
            let status = TISSelectInputSource(target)
            guard status == noErr else { throw DriverError.sourceSelectionFailed(status) }
            let until = Date().addingTimeInterval(2)
            while Date() < until && currentSource().map(sourceID) != requested {
                Thread.sleep(forTimeInterval: 0.05)
            }
            guard currentSource().map(sourceID) == requested else {
                throw DriverError.sourceDidNotChange(requested)
            }
            emit("selected", ["id": requested, "alreadyEnabled": wasEnabled])
            if let signal = runtime.outcome().1 {
                throw DriverError.invalidArguments("Interrupted before child launch: signal \(signal)")
            }
            try runtime.child.run()
            if let keyDriver {
                keys.executableURL = URL(fileURLWithPath: keyDriver)
                keys.arguments = ["--pid", String(runtime.child.processIdentifier), "--mode", keyMode, "--start-delay", "2.0", "--transport", keyTransport, "--exercise-viewport", exerciseViewport]
                if let candidateOutput { keys.arguments! += ["--candidate-output", candidateOutput] }
                keys.standardOutput = FileHandle.standardOutput
                keys.standardError = FileHandle.standardError
                try keys.run()
            }
            // App activation can restore that app's previous keyboard source.
            // Re-select after its window has become active, before the probe's
            // delayed first key. This is a main-thread TIS operation.
            let activationDeadline = Date().addingTimeInterval(keyDriver == nil ? 1.5 : 5)
            while Date() < activationDeadline {
                if NSRunningApplication(processIdentifier: runtime.child.processIdentifier)?.isActive == true { break }
                Thread.sleep(forTimeInterval: 0.025)
            }
            if currentSource().map(sourceID) != requested {
                let selected = TISSelectInputSource(target)
                guard selected == noErr else { throw DriverError.sourceSelectionFailed(selected) }
            }
            emit("activeSource", ["id": currentSource().map(sourceID) ?? "<missing>", "childActive": NSRunningApplication(processIdentifier: runtime.child.processIdentifier)?.isActive == true])
            if let signal = runtime.outcome().1 { runtime.stop(signal: signal) }
            DispatchQueue.global().asyncAfter(deadline: .now() + timeout) { runtime.stop(timedOut: true) }
            runtime.child.waitUntilExit()
            if keyDriver != nil { keys.waitUntilExit() }
            let (timedOut, signal) = runtime.outcome()
            result = timedOut ? 124 : signal.map { 128 + $0 } ?? runtime.child.terminationStatus
            if keyDriver != nil && keys.terminationStatus != 0 { result = 4 }
            emit("childExit", ["status": runtime.child.terminationStatus, "timedOut": timedOut, "interrupted": signal != nil])
        } catch {
            FileHandle.standardError.write(Data("INPUT_SOURCE_ERROR \(error)\n".utf8))
            result = runtime.outcome().1.map { 128 + $0 } ?? 1
        }
        if !restore.restored { return 3 }
        return result
    } catch {
        FileHandle.standardError.write(Data("INPUT_SOURCE_ERROR \(error)\n".utf8))
        return 1
    }
}

Darwin.exit(run())
