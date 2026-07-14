import SwiftUI

extension View {
    func commandPalette(isPresented: Binding<Bool>, shell: ShellModel) -> some View {
        modifier(CommandPalettePresenter(isPresented: isPresented, shell: shell))
    }
}

private struct CommandPalettePresenter: ViewModifier {
    @Binding var isPresented: Bool
    let shell: ShellModel
    @State private var model = CommandPaletteModel()
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    func body(content: Content) -> some View {
        content
            .overlay {
                Group {
                    if isPresented {
                        Design.shadowColor.opacity(0.24)
                            .ignoresSafeArea()
                            .onTapGesture { isPresented = false }
                            .accessibilityLabel("Dismiss")
                            .transition(.opacity)
                    }
                }
                .animation(.easeOut(duration: 0.15), value: isPresented)
            }
            .overlay(alignment: .top) {
                Group {
                    if isPresented {
                        CommandPaletteView(
                            model: model,
                            onClose: { isPresented = false },
                            onPerform: { item in
                                item.perform()
                                isPresented = false
                            }
                        )
                        .padding(.top, 96)
                        .onExitCommand { isPresented = false }
                        .onAppear { model.reload(for: shell) }
                        .transition(
                            reduceMotion
                                ? .opacity
                                : .opacity
                                    .combined(with: .scale(scale: 0.96, anchor: .top))
                                    .combined(with: .offset(y: -12)))
                    }
                }
                .animation(
                    Design.reveal(isPresented, reduceMotion: reduceMotion),
                    value: isPresented)
            }
    }
}
