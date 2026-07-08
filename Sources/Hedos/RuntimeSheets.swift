import AppKit
import HedosKernel
import SwiftUI

enum RuntimePicker {
    static func pick(_ onPick: @escaping (URL) -> Void) {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = true
        panel.allowedContentTypes = []
        panel.allowsMultipleSelection = false
        panel.prompt = "Review"
        panel.message = "Pick a runtime folder or manifest.toml"
        if panel.runModal() == .OK, let url = panel.url {
            onPick(url)
        }
    }
}

struct InstallRuntimeSheet: View {
    let shell: ShellModel
    let source: URL
    let onClose: () -> Void

    @State private var preview: RuntimeInstallPreview?
    @State private var failure: String?
    @State private var installing = false
    @State private var installed = false

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
                .padding(.horizontal, Design.Space.gutter)
                .padding(.top, Design.Space.gutter)
                .padding(.bottom, Design.Space.xl)
            Rectangle().fill(Design.hairline).frame(height: Design.hairlineWidth)
            ScrollView {
                VStack(alignment: .leading, spacing: Design.Space.xl) {
                    if let failure {
                        HStack(spacing: Design.Space.s) {
                            Image(systemName: "exclamationmark.triangle")
                                .font(Design.glyphInline)
                                .foregroundStyle(Design.inkSoft)
                            Text(failure)
                                .font(Design.caption.weight(.medium))
                                .foregroundStyle(Design.inkSoft)
                        }
                    } else if let preview {
                        diff(preview)
                    } else {
                        Text("Reading the manifest…")
                            .font(Design.label)
                            .foregroundStyle(Design.inkFaint)
                    }
                }
                .padding(.horizontal, Design.Space.gutter)
                .padding(.vertical, Design.Space.xl)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            Rectangle().fill(Design.hairline).frame(height: Design.hairlineWidth)
            footer
                .padding(.horizontal, Design.Space.gutter)
                .padding(.vertical, Design.Space.l)
        }
        .frame(width: Design.Sheet.serverWidth)
        .frame(maxHeight: Design.Sheet.serverHeight)
        .task {
            do {
                preview = try await shell.settings.previewRuntimeInstall(from: source)
            } catch {
                failure = (error as? ManifestValidationError)?.message
                    ?? error.localizedDescription
            }
        }
    }

    private var header: some View {
        HStack(alignment: .center, spacing: Design.Space.l) {
            IconPlaque(size: 44) {
                Image(systemName: "shippingbox")
                    .font(Design.glyphNav)
                    .foregroundStyle(Design.inkSoft)
            }
            VStack(alignment: .leading, spacing: Design.Space.xxs) {
                Text(installed ? "Installed" : "Install a runtime")
                    .font(Design.title)
                    .tracking(Design.tightTracking)
                Text(
                    installed
                        ? "Matching models resolve to it on the next scan"
                        : "Everything it may touch, before it runs"
                )
                .font(Design.label)
                .foregroundStyle(Design.inkFaint)
            }
            Spacer()
            SheetCloseButton(action: onClose)
        }
    }

    @ViewBuilder
    private func diff(_ preview: RuntimeInstallPreview) -> some View {
        VStack(alignment: .leading, spacing: Design.Space.m) {
            MicroHeader(title: preview.id)
            VStack(alignment: .leading, spacing: Design.Space.m) {
                diffRow(
                    "shield.lefthalf.filled", "Runs contained",
                    "Inside its own Linux machine — no network when it runs, ever.")
                if let detect = preview.detectSummary {
                    diffRow("scope", "Serves", detect)
                }
                diffRow(
                    "square.stack.3d.up", "Capabilities",
                    preview.capabilities.joined(separator: ", "))
                diffRow("shippingbox", "Machine image", preview.image)
                if !preview.setup.isEmpty {
                    diffRow(
                        "arrow.down.circle", "One-time setup with network",
                        preview.setup.joined(separator: "\n"))
                }
                diffRow(
                    "folder", "Files it sees",
                    "The model (read-only), its own work folder, and its outputs — nothing else.")
                if let download = preview.vmAssetDownloadMB {
                    diffRow(
                        "arrow.down.to.line", "First use",
                        "Downloads a ~\(download) MB Linux kernel, verified against a pinned checksum.")
                }
            }
            .padding(Design.Space.tile)
            .surfaceCard(radius: Design.Radius.tile)
        }
    }

    private func diffRow(_ glyph: String, _ title: String, _ detail: String) -> some View {
        HStack(alignment: .top, spacing: Design.Space.m) {
            Image(systemName: glyph)
                .font(Design.glyphInline)
                .foregroundStyle(Design.inkSoft)
                .frame(width: 18)
            VStack(alignment: .leading, spacing: Design.Space.xxs) {
                Text(title)
                    .font(Design.caption.weight(.medium))
                    .foregroundStyle(Design.ink)
                Text(detail)
                    .font(Design.label)
                    .foregroundStyle(Design.inkFaint)
                    .lineSpacing(1.5)
                    .fixedSize(horizontal: false, vertical: true)
                    .textSelection(.enabled)
            }
        }
    }

    private var footer: some View {
        HStack {
            Spacer()
            if installed {
                Button("Done") { onClose() }
                    .buttonStyle(InkButtonStyle())
                    .keyboardShortcut(.defaultAction)
            } else {
                Button(installing ? "Installing…" : "Install") {
                    install()
                }
                .buttonStyle(InkButtonStyle())
                .keyboardShortcut(.defaultAction)
                .disabled(preview == nil || installing)
                .accessibilityIdentifier("runtime-install-confirm")
            }
        }
    }

    private func install() {
        installing = true
        let shell = shell
        let source = source
        Task { @MainActor in
            do {
                try await shell.settings.installRuntime(from: source)
                await shell.library.rescan()
                installed = true
            } catch {
                failure = (error as? ManifestValidationError)?.message
                    ?? error.localizedDescription
            }
            installing = false
        }
    }
}
