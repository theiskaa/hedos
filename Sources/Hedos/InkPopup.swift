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
    @ViewBuilder let content: () -> Content
    @State private var measured: CGFloat = 0

    var body: some View {
        let height = min(max(measured, 1), maxHeight)
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
        .onPreferenceChange(PopoverHeightKey.self) { measured = $0 }
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
            InkPopoverBody(width: width, maxHeight: maxHeight, content: content)
        }
    }
}
