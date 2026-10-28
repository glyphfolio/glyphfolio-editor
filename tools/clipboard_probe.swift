// Native cross-process clipboard test. Prior representations remain only in memory.
// Do not print clipboard payloads or invoke this while a user is actively copying.
import AppKit
import Foundation

enum ProbeError: Error { case failed(String) }
func require(_ condition: Bool, _ message: String) throws {
    if !condition { throw ProbeError.failed(message) }
}
func invoke(_ helper: String, _ mode: String, _ token: String) throws {
    let process = Process()
    process.executableURL = URL(fileURLWithPath: helper)
    process.arguments = [mode, token]
    let output = Pipe()
    process.standardOutput = output
    process.standardError = output
    try process.run()
    let bytes = output.fileHandleForReading.readDataToEndOfFile()
    process.waitUntilExit()
    let expected = mode == "write" ? "WRITE_OK\n" : "READ_OK\n"
    try require(process.terminationStatus == 0 && String(data: bytes, encoding: .utf8) == expected,
                "native helper failed; payload output suppressed")
}

let helper = CommandLine.arguments.dropFirst().first ?? ""
let board = NSPasteboard.general
var previous: [[(NSPasteboard.PasteboardType, Data)]] = []
var total = 0
var ownedChange: Int? = nil
var probeSucceeded = false
do {
    try require(!helper.isEmpty, "expected native helper path")
    // Resolve all representations BEFORE changing anything. Refuse a partial or
    // oversized backup rather than losing a promised clipboard representation.
    let capturedCount = board.changeCount
    for item in board.pasteboardItems ?? [] {
        var representations: [(NSPasteboard.PasteboardType, Data)] = []
        for type in item.types {
            guard let data = item.data(forType: type) else {
                throw ProbeError.failed("cannot preserve a clipboard representation; clipboard unchanged")
            }
            total += data.count
            try require(total <= 32 * 1024 * 1024, "clipboard backup exceeds test limit; clipboard unchanged")
            representations.append((type, data))
        }
        previous.append(representations)
    }
    try require(board.changeCount == capturedCount, "clipboard changed during capture; test cancelled")
    let token = "glyphfolio-clipboard-" + UUID().uuidString
    let expected = "**\(token)**\r\n日本 é 👩‍👩‍👧‍👦\n"
    do {
        try invoke(helper, "write", token)
    } catch {
        if board.string(forType: .string) == expected || (board.pasteboardItems ?? []).isEmpty {
            ownedChange = board.changeCount
        }
        throw error
    }
    ownedChange = board.changeCount
    try require(board.string(forType: .string) == expected, "Rust-to-AppKit clipboard mismatch")
    try require(board.changeCount == ownedChange, "clipboard owner changed during probe")
    board.clearContents()
    ownedChange = board.changeCount
    let published = board.setString(expected, forType: .string)
    ownedChange = board.changeCount
    try require(published, "AppKit clipboard publication failed")
    try invoke(helper, "read", token)
    try require(board.changeCount == ownedChange, "clipboard owner changed during readback")
    print("NATIVE_CLIPBOARD transport=PASS directions=2 exact_utf8_crlf=PASS")
    probeSucceeded = true
} catch {
    // Errors contain no clipboard representations or synthetic/user text.
    fputs("NATIVE_CLIPBOARD probe=FAIL \(error)\n", stderr)
}
if let owned = ownedChange {
    if board.changeCount == owned {
        let restored: [NSPasteboardItem] = previous.map { representations in
            let item = NSPasteboardItem()
            for (type, data) in representations { item.setData(data, forType: type) }
            return item
        }
        board.clearContents()
        let success = restored.isEmpty || board.writeObjects(restored)
        var same = success && (board.pasteboardItems ?? []).count == previous.count
        if same {
            for (index, item) in (board.pasteboardItems ?? []).enumerated() {
                for (type, data) in previous[index] {
                    same = same && item.data(forType: type) == data
                }
            }
        }
        print("CLIPBOARD_REPRESENTATIONS_RESTORED=\(same)")
        if !same { exit(2) }
    } else {
        print("RESTORE_SKIPPED_NEW_CLIPBOARD_OWNER")
        exit(3)
    }
}
if !probeSucceeded { exit(1) }
