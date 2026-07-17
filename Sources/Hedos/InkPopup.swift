import SwiftUI

private struct PopoverHeightKey: PreferenceKey {
    static let defaultValue: CGFloat = 0
    static func reduce(value: inout CGFloat, nextValue: () -> CGFloat) {
        value = max(value, nextValue())
    }
}

struct InkPopoverBody<Content: View>: View {
    let width: CGFloat
    let maxHeight: CGFloat
    var onDismiss: (@MainActor () -> Void)? = nil
    @ViewBuilder let content: () -> Content
    @State private var measured: CGFloat = 0
    @State private var nav = KeyNavCoordinator()
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        let height = min(max(measured, 1), maxHeight)
        ScrollViewReader { proxy in
            ScrollView {
                content()
                    .frame(width: width, alignment: .leading)
                    .background(
                        GeometryReader { geometry in
                            Color.clear.preference(
                                key: PopoverHeightKey.self, value: geometry.size.height)
                        })
            }
            .scrollDisabled(measured <= maxHeight)
            .frame(width: width, height: height)
            .onPreferenceChange(PopoverHeightKey.self) { value in
                if measured > 0 {
                    withAnimation(Design.wash) { measured = value }
                } else {
                    measured = value
                }
            }
            .onChange(of: nav.highlightedID) { previous, id in
                guard let id else { return }
                if reduceMotion || previous == nil {
                    proxy.scrollTo(id, anchor: .center)
                } else {
                    withAnimation(Design.snap) {
                        proxy.scrollTo(id, anchor: .center)
                    }
                }
            }
        }
        .keyNavigableList(coordinator: nav, onDismiss: onDismiss)
        .background(Design.panel)
        .presentationBackground(Design.panel)
    }
}

extension View {
    func inkPopover<PopoverContent: View>(
        isPresented: Binding<Bool>, width: CGFloat, maxHeight: CGFloat,
        @ViewBuilder content: @escaping () -> PopoverContent
    ) -> some View {
        popover(isPresented: isPresented, arrowEdge: .top) {
            InkPopoverBody(
                width: width, maxHeight: maxHeight,
                onDismiss: { isPresented.wrappedValue = false },
                content: content)
        }
    }
}
