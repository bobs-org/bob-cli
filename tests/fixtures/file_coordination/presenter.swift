// Test-only windowless NSDocument. No custom coordination/save/reload logic:
// AppKit must save dirty contents and reload after a .forMerging writer.
import AppKit
import CoreGraphics

func reply(_ value: [String: Any]) {
    let data = try! JSONSerialization.data(withJSONObject: value, options: [.sortedKeys])
    FileHandle.standardOutput.write(data)
    FileHandle.standardOutput.write(Data([10]))
}

final class Document: NSDocument {
    var text = ""
    var reads = 0

    override class var autosavesInPlace: Bool { true }

    override func read(from data: Data, ofType typeName: String) throws {
        guard let contents = String(data: data, encoding: .utf8) else {
            throw NSError(domain: "Fixture", code: 1)
        }
        text = contents
        reads += 1
    }

    override func data(ofType typeName: String) throws -> Data {
        Data(text.utf8)
    }

    func state() -> [String: Any] {
        ["event": "state", "text": text, "dirty": isDocumentEdited, "reads": reads]
    }
}

guard CGSessionCopyCurrentDictionary() != nil else {
    reply(["event": "skip", "reason": "no macOS login session for NSDocument"])
    exit(0)
}
let app = NSApplication.shared
app.setActivationPolicy(.prohibited)
NSDocumentController.shared.autosavingDelay = 3600
let document: Document
do {
    document = try Document(contentsOf: URL(fileURLWithPath: CommandLine.arguments[1]), ofType: "public.plain-text")
} catch {
    reply(["event": "error", "reason": "cannot open fixture document: \(error)"])
    exit(1)
}
NSDocumentController.shared.addDocument(document)
guard NSFileCoordinator.filePresenters.contains(where: { ($0 as AnyObject) === document }) else {
    reply(["event": "skip", "reason": "AppKit did not register the NSDocument file presenter"])
    exit(0)
}
reply(["event": "ready"])

DispatchQueue.global().async {
    while let command = readLine() {
        DispatchQueue.main.async {
            if command.hasPrefix("edit ") {
                document.text += String(command.dropFirst(5))
                document.updateChangeCount(.changeDone)
                reply(document.state())
            } else if command.hasPrefix("block "), let seconds = Double(command.dropFirst(6)) {
                reply(["event": "blocked"])
                Thread.sleep(forTimeInterval: seconds)
                reply(["event": "unblocked"])
            } else if command == "state" {
                reply(document.state())
            } else if command == "save" {
                document.save(to: document.fileURL!, ofType: "public.plain-text", for: .saveOperation) { error in
                    if let error {
                        reply(["event": "error", "reason": "save failed: \(error)"])
                    } else {
                        reply(["event": "saved"])
                    }
                }
            } else if command == "quit" {
                exit(0)
            } else {
                reply(["event": "error", "reason": "unknown command: \(command)"])
            }
        }
    }
    exit(0)
}
app.run()
