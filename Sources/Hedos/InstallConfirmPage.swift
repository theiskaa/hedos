import HedosKernel
import SwiftUI

struct InstallConfirmPage: View {
    @Bindable var shell: ShellModel
    let plan: InstallPlan

    @State private var beginning = false

    private var installs: InstallModel { shell.installs }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            ScrollView {
                VStack(alignment: .leading, spacing: Design.Space.xl) {
                    statsStrip
                    if plan.files.isEmpty {
                        pullNote
                    } else {
                        detailsCard
                        filesSection
                    }
                    if let error = installs.stageError {
                        Text(error)
                            .font(Design.label)
                            .foregroundStyle(Design.heatText)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
                .padding(.horizontal, Design.Space.gutter)
                .padding(.vertical, Design.Space.xl)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            Rectangle().fill(Design.hairline).frame(height: Design.hairlineWidth)
            HStack(spacing: Design.Space.l) {
                if plan.requiresAuth {
                    Text("Gated model — it needs a Hugging Face token first.")
                        .font(Design.label)
                        .foregroundStyle(Design.heatText)
                        .lineLimit(2)
                    Button("Open Settings…") {
                        shell.openSettings(
                            at: SettingsDestination(section: .models, anchor: "models.hfToken"))
                    }
                    .buttonStyle(QuietButtonStyle())
                    Button("Check again") {
                        Task {
                            await installs.stage(
                                provider: plan.provider, reference: plan.reference)
                        }
                    }
                    .buttonStyle(QuietButtonStyle())
                    .disabled(installs.stagingID != nil)
                }
                Spacer()
                Button(beginning ? "Starting…" : "Install") {
                    beginning = true
                    Task {
                        _ = await installs.confirm(plan)
                        beginning = false
                    }
                }
                .buttonStyle(InkButtonStyle())
                .keyboardShortcut(.defaultAction)
                .disabled(beginning || plan.requiresAuth)
                .accessibilityIdentifier("model-install-confirm")
            }
            .padding(.horizontal, Design.Space.gutter)
            .padding(.vertical, Design.Space.l)
        }
    }

    private var statsStrip: some View {
        HStack(alignment: .top, spacing: 0) {
            stat(label: "Download", value: sizeValue, detail: sizeDetail)
            statDivider
            if plan.files.isEmpty {
                stat(label: "Tag", value: tagValue, detail: "pulled layer by layer")
            } else {
                stat(
                    label: "Files", value: "\(plan.files.count)",
                    detail: "\(weightFiles.count) weight\(weightFiles.count == 1 ? "" : "s"), the rest configs")
            }
            statDivider
            stat(label: "Source", value: providerName, detail: sourceDetail)
        }
        .padding(.vertical, Design.Space.l)
        .surfaceCard(radius: Design.Radius.tile)
    }

    private func stat(label: String, value: String, detail: String) -> some View {
        VStack(alignment: .leading, spacing: Design.Space.xs) {
            Text(label.uppercased())
                .font(Design.micro)
                .tracking(Design.microTracking)
                .foregroundStyle(Design.inkFaint)
            Text(value)
                .font(Design.data(16))
                .monospacedDigit()
                .foregroundStyle(Design.ink)
                .lineLimit(1)
                .truncationMode(.middle)
            Text(detail)
                .font(Design.label)
                .foregroundStyle(Design.inkFaint)
                .lineLimit(1)
        }
        .padding(.horizontal, Design.Space.xl)
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var statDivider: some View {
        Rectangle()
            .fill(Design.hairline)
            .frame(width: Design.hairlineWidth)
            .padding(.vertical, Design.Space.xxs)
    }

    private var detailsCard: some View {
        VStack(alignment: .leading, spacing: Design.Space.m) {
            confirmRow(
                "folder", "Lands in",
                "\(plan.destination) — the same hub cache huggingface tooling reads, blobs verified against the pinned revision.")
            if plan.requiresAuth {
                confirmRow(
                    "lock", "Gated model",
                    "The owner requires an access token before files download. Nothing starts until one is set.",
                    heat: true)
            }
        }
        .padding(Design.Space.tile)
        .frame(maxWidth: .infinity, alignment: .leading)
        .surfaceCard(radius: Design.Radius.tile)
    }

    private var filesSection: some View {
        VStack(alignment: .leading, spacing: Design.Space.m) {
            MicroHeader(title: "What downloads")
            VStack(spacing: 0) {
                let ordered = orderedFiles
                ForEach(Array(ordered.enumerated()), id: \.element.path) { index, file in
                    HStack(spacing: Design.Space.m) {
                        Text(file.path)
                            .font(Design.data(11))
                            .foregroundStyle(Design.ink)
                            .lineLimit(1)
                            .truncationMode(.middle)
                            .textSelection(.enabled)
                        if file.isWeight {
                            TintChip(text: "weights")
                        }
                        Spacer(minLength: Design.Space.m)
                        Text(file.bytes.map { ByteFormat.string($0) } ?? "—")
                            .font(Design.data(11))
                            .monospacedDigit()
                            .foregroundStyle(Design.inkFaint)
                    }
                    .padding(.horizontal, Design.Space.m)
                    .padding(.vertical, Design.Space.s + 1)
                    if index < ordered.count - 1 {
                        Rectangle().fill(Design.hairline)
                            .frame(height: Design.hairlineWidth)
                            .padding(.leading, Design.Space.m)
                    }
                }
            }
            .surfaceCard(radius: Design.Radius.tile)
        }
    }

    private var pullNote: some View {
        VStack(alignment: .leading, spacing: Design.Space.m) {
            MicroHeader(title: "How it lands")
            VStack(alignment: .leading, spacing: Design.Space.m) {
                confirmRow(
                    "arrow.down.circle", "Pulled by Ollama itself",
                    "hedos asks the local daemon to pull this tag, the same request `ollama pull` makes. Layer sizes and progress appear the moment the transfer starts.")
                confirmRow(
                    "square.stack.3d.up", "Straight into Ollama's store",
                    "Layers land in \(plan.destination) where every other Ollama tool can use them. Cancel any time — finished layers stay and the next pull resumes from them.")
                confirmRow(
                    "checkmark.circle", "On your shelf when done",
                    "The scanner watches Ollama's store, so the model registers and resolves without a manual rescan.")
            }
            .padding(Design.Space.tile)
            .frame(maxWidth: .infinity, alignment: .leading)
            .surfaceCard(radius: Design.Radius.tile)
        }
    }

    private var orderedFiles: [InstallPlanFile] {
        plan.files.sorted { first, second in
            let firstWeight = first.isWeight
            let secondWeight = second.isWeight
            if firstWeight != secondWeight {
                return firstWeight
            }
            if firstWeight, first.bytes != second.bytes {
                return (first.bytes ?? 0) > (second.bytes ?? 0)
            }
            return first.path < second.path
        }
    }

    private var weightFiles: [InstallPlanFile] {
        plan.files.filter(\.isWeight)
    }

    private var providerName: String {
        installs.providers.first { $0.id == plan.provider }?.displayName
            ?? plan.provider.rawValue
    }

    private var tagValue: String {
        plan.reference.split(separator: ":").last.map(String.init) ?? plan.reference
    }

    private var sourceDetail: String {
        if plan.provider == .ollama {
            return "through the local daemon"
        }
        return plan.revision.map { "pinned to \(String($0.prefix(7)))" } ?? "hub resolve"
    }

    private var sizeValue: String {
        if let total = plan.totalBytes {
            return ByteFormat.string(total)
        }
        if let estimate = catalogEstimateGB {
            return String(format: "≈ %g GB", estimate)
        }
        return "—"
    }

    private var sizeDetail: String {
        if let total = plan.totalBytes {
            if let remaining = plan.remainingBytes, remaining < total {
                return "\(ByteFormat.string(remaining)) to go, the rest is here"
            }
            return "verified as it downloads"
        }
        return "exact size once the pull starts"
    }

    private var catalogEstimateGB: Double? {
        InstallCatalog.entries.first {
            $0.provider == plan.provider && $0.reference == plan.reference
        }?.sizeGB
    }

    private func confirmRow(
        _ glyph: String, _ title: String, _ detail: String, heat: Bool = false
    ) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: Design.Space.m) {
            Image(systemName: glyph)
                .font(Design.glyphInline)
                .foregroundStyle(heat ? Design.heatText : Design.inkSoft)
                .frame(width: 18)
            VStack(alignment: .leading, spacing: Design.Space.xxs) {
                Text(title)
                    .font(Design.caption.weight(.medium))
                    .foregroundStyle(heat ? Design.heatText : Design.ink)
                Text(detail)
                    .font(Design.label)
                    .foregroundStyle(Design.inkFaint)
                    .lineSpacing(1.5)
                    .fixedSize(horizontal: false, vertical: true)
                    .textSelection(.enabled)
            }
        }
    }
}
