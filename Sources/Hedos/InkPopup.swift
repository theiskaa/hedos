import AppKit
import SwiftUI

struct AnchorReader: NSViewRepresentable {
    let store: (NSView) -> Void

    func makeNSView(context: Context) -> NSView {
        let view = NSView(frame: .zero)
        DispatchQueue.main.async { store(view) }
        return view
    }

    func updateNSView(_ nsView: NSView, context: Context) {
        store(nsView)
    }
}

@MainActor
final class InkPopup {
    static let shared = InkPopup()

    private var panel: NSPanel?
    private var monitor: Any?
    private var dismissHandler: (() -> Void)?

    func present<Content: View>(
        anchor: NSView, width: CGFloat, maxHeight: CGFloat, content: Content,
        onDismiss: @escaping () -> Void
    ) {
        dismiss()
        guard let window = anchor.window else { return }
        dismissHandler = onDismiss

        let measure = NSHostingController(rootView: content.frame(width: width))
        measure.view.layoutSubtreeIfNeeded()
        let contentHeight = measure.view.fittingSize.height
        let scrolls = contentHeight > maxHeight
        let panelHeight = max(1, min(contentHeight, maxHeight))

        let styled = AnyView(
            Group {
                if scrolls {
                    ScrollView { content }.frame(width: width, height: panelHeight)
                } else {
                    content.frame(width: width, height: panelHeight)
                }
            }
            .background(Design.panel)
            .overlay(
                RoundedRectangle(cornerRadius: Design.Radius.control)
                    .strokeBorder(Design.line, lineWidth: Design.hairlineWidth)
            )
            .clipShape(RoundedRectangle(cornerRadius: Design.Radius.control))
        )

        let local = anchor.convert(anchor.bounds, to: nil)
        let screenRect = window.convertToScreen(local)
        var origin = NSPoint(x: screenRect.minX, y: screenRect.minY - panelHeight - 4)
        if let visible = window.screen?.visibleFrame, origin.y < visible.minY {
            origin.y = screenRect.maxY + 4
        }

        let panel = NSPanel(
            contentRect: NSRect(origin: origin, size: NSSize(width: width, height: panelHeight)),
            styleMask: [.borderless, .nonactivatingPanel],
            backing: .buffered,
            defer: false)
        panel.isFloatingPanel = true
        panel.level = .popUpMenu
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.hasShadow = true
        panel.isReleasedWhenClosed = false
        panel.hidesOnDeactivate = true
        panel.appearance = window.appearance
        panel.contentViewController = NSHostingController(rootView: styled)
        panel.setContentSize(NSSize(width: width, height: panelHeight))
        panel.orderFrontRegardless()
        self.panel = panel

        monitor = NSEvent.addLocalMonitorForEvents(matching: [.leftMouseDown, .rightMouseDown]) {
            [weak self] event in
            if event.window !== panel {
                self?.dismiss()
            }
            return event
        }
    }

    func dismiss() {
        if let monitor {
            NSEvent.removeMonitor(monitor)
            self.monitor = nil
        }
        panel?.orderOut(nil)
        panel = nil
        let handler = dismissHandler
        dismissHandler = nil
        handler?()
    }
}

extension View {
    func inkPopover<PopoverContent: View>(
        isPresented: Binding<Bool>, width: CGFloat, maxHeight: CGFloat,
        @ViewBuilder content: @escaping () -> PopoverContent
    ) -> some View {
        modifier(
            InkPopoverModifier(
                isPresented: isPresented, width: width, maxHeight: maxHeight, content: content))
    }
}

private struct InkPopoverModifier<PopoverContent: View>: ViewModifier {
    @Binding var isPresented: Bool
    let width: CGFloat
    let maxHeight: CGFloat
    @ViewBuilder let content: () -> PopoverContent
    @State private var anchor: NSView?

    func body(content base: Content) -> some View {
        base
            .background(AnchorReader { anchor = $0 })
            .onChange(of: isPresented) { _, show in
                if show { present() }
            }
    }

    private func present() {
        guard let anchor else { return }
        InkPopup.shared.present(
            anchor: anchor, width: width, maxHeight: maxHeight, content: content(),
            onDismiss: { isPresented = false })
    }
}
