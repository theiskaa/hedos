import AppKit
import HedosKernel
import SwiftUI

@MainActor
final class MenuBarController {
    static let shared = MenuBarController()
    weak var shell: ShellModel?
    private var item: NSStatusItem?
    private var activityTask: Task<Void, Never>?

    func apply(_ enabled: Bool) {
        if enabled {
            guard item == nil else { return }
            let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
            item.button?.image = Self.icon()
            item.menu = buildMenu()
            self.item = item
            watchActivity()
        } else {
            if let item {
                NSStatusBar.system.removeStatusItem(item)
            }
            item = nil
            activityTask?.cancel()
            activityTask = nil
        }
    }

    private func buildMenu() -> NSMenu {
        let menu = NSMenu()
        let show = NSMenuItem(
            title: "Show Hedos", action: #selector(showApp), keyEquivalent: "")
        show.target = self
        menu.addItem(show)
        let ask = NSMenuItem(
            title: "Quick Ask", action: #selector(quickAsk), keyEquivalent: "")
        ask.target = self
        menu.addItem(ask)
        menu.addItem(.separator())
        let quit = NSMenuItem(
            title: "Quit Hedos", action: #selector(NSApplication.terminate(_:)),
            keyEquivalent: "q")
        menu.addItem(quit)
        return menu
    }

    @objc private func showApp() {
        NSApp.activate(ignoringOtherApps: true)
        NSApp.windows.first { $0.styleMask.contains(.fullSizeContentView) && !($0 is NSPanel) }?
            .makeKeyAndOrderFront(nil)
    }

    @objc private func quickAsk() {
        QuickAskController.shared.toggle()
    }

    private var activityLayer: CALayer?

    private func watchActivity() {
        activityTask?.cancel()
        activityTask = Task { [weak self] in
            while !Task.isCancelled {
                guard let self, let shell = self.shell else { return }
                let active = await shell.kernel.scheduler.active().count > 0
                self.setActivityDot(visible: active)
                try? await Task.sleep(for: .seconds(3))
            }
        }
    }

    private func setActivityDot(visible: Bool) {
        guard let button = item?.button else { return }
        if visible {
            guard activityLayer == nil else { return }
            button.wantsLayer = true
            let dot = CALayer()
            dot.backgroundColor = NSColor(Design.accent).cgColor
            dot.cornerRadius = 2.5
            dot.frame = CGRect(
                x: button.bounds.maxX - 9, y: button.bounds.minY + 3, width: 5, height: 5)
            let blink = CABasicAnimation(keyPath: "opacity")
            blink.fromValue = 1.0
            blink.toValue = 0.25
            blink.duration = 0.9
            blink.autoreverses = true
            blink.repeatCount = .infinity
            if !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion {
                dot.add(blink, forKey: "blink")
            }
            button.layer?.addSublayer(dot)
            activityLayer = dot
        } else {
            activityLayer?.removeFromSuperlayer()
            activityLayer = nil
        }
    }

    private static func icon() -> NSImage {
        let size = NSSize(width: 18, height: 18)
        let image = NSImage(size: size, flipped: false) { rect in
            let path = NSBezierPath()
            let center = NSPoint(x: rect.midX, y: rect.midY)
            let radius = min(rect.width, rect.height) * 0.42
            for index in 0..<7 {
                let angle = (Double(index) * 2 * .pi / 7) + .pi / 2
                let point = NSPoint(
                    x: center.x + radius * cos(angle),
                    y: center.y + radius * sin(angle))
                if index == 0 {
                    path.move(to: point)
                } else {
                    path.line(to: point)
                }
            }
            path.close()
            NSColor.black.setFill()
            path.fill()
            return true
        }
        image.isTemplate = true
        return image
    }
}

