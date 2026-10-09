// PROTOTYPE (#18): plain AppKit floor. One 1200x800 window, nothing drawn.
// Prints ms since process start (kernel's record) at each step, then quits.
import AppKit
import Darwin

func sinceStart() -> Double {
    var info = rusage_info_v4()
    _ = withUnsafeMutablePointer(to: &info) {
        $0.withMemoryRebound(to: rusage_info_t?.self, capacity: 1) { proc_pid_rusage(getpid(), RUSAGE_INFO_V4, $0) }
    }
    var tb = mach_timebase_info_data_t(); mach_timebase_info(&tb)
    return Double(mach_absolute_time() - info.ri_proc_start_abstime) * Double(tb.numer) / Double(tb.denom) / 1e6
}
var marks: [(String, Double)] = [("main", sinceStart())]
func mark(_ s: String) { marks.append((s, sinceStart())) }

class Delegate: NSObject, NSApplicationDelegate {
    var window: NSWindow!
    func applicationDidFinishLaunching(_ n: Notification) {
        mark("did_finish_launching")
        window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 1200, height: 800),
                          styleMask: [.titled, .closable, .resizable, .miniaturizable], backing: .buffered, defer: false)
        window.title = "floor-appkit (PROTOTYPE)"
        NotificationCenter.default.addObserver(forName: NSWindow.didChangeOcclusionStateNotification, object: window, queue: nil) { _ in
            if self.window.occlusionState.contains(.visible) {
                mark("visible")
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) {
                    print("{" + marks.map { "\"\($0.0)\": \(String(format: "%.2f", $0.1))" }.joined(separator: ", ") + "}")
                    exit(0)
                }
            }
        }
        window.makeKeyAndOrderFront(nil)
        mark("ordered_front")
        NSApp.activate(ignoringOtherApps: true)
    }
}
let app = NSApplication.shared
mark("shared_application")
let d = Delegate(); app.delegate = d
app.setActivationPolicy(.regular)
app.run()
