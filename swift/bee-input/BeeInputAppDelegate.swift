import Cocoa
import InputMethodKit

private func beeLogPath() -> String {
    FileManager.default.containerURL(
        forSecurityApplicationGroupIdentifier: "B2N6FSRTPV.group.fasterthanlime.bee"
    )?.appendingPathComponent("bee.log").path ?? "/tmp/bee.log"
}

func beeInputLog(_ msg: String) {
    let path = beeLogPath()
    let ts = ProcessInfo.processInfo.systemUptime
    let line = String(format: "[%.3f] IME: %@\n", ts, msg)
    if let data = line.data(using: .utf8),
        let fh = FileHandle(forWritingAtPath: path)
    {
        fh.seekToEndOfFile()
        fh.write(data)
        fh.closeFile()
    } else if let data = line.data(using: .utf8) {
        try? data.write(to: URL(fileURLWithPath: path))
    }
}

class BeeInputAppDelegate: NSObject, NSApplicationDelegate {
    var server: IMKServer?

    func applicationDidFinishLaunching(_ notification: Notification) {
        guard
            let connectionName = Bundle.main.infoDictionary?["InputMethodConnectionName"]
                as? String,
            let bundleIdentifier = Bundle.main.bundleIdentifier
        else {
            beeInputLog("failed to initialize IMKServer: missing bundle metadata")
            return
        }

        server = IMKServer(
            name: connectionName,
            bundleIdentifier: bundleIdentifier
        )
        beeInputLog("IMK server started")
        BeeVoxIMEClient.shared.start()
    }
}
