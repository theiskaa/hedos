@preconcurrency import AVFoundation
import HedosKernel
import SwiftUI

@Observable
@MainActor
final class PipelinesModel {
    let kernel: Kernel
    var pipelines: [Pipeline] = []
    var signatures: [String: PipelineSignature] = [:]
    var issues: [String: String] = [:]

    init(kernel: Kernel) {
        self.kernel = kernel
    }

    func refresh() async {
        pipelines = await kernel.pipelineStore.list()
        var resolved: [String: PipelineSignature] = [:]
        var problems: [String: String] = [:]
        for pipeline in pipelines {
            if let signature = await kernel.pipelineStore.signature(of: pipeline) {
                resolved[pipeline.id] = signature
            } else if let diagnostic = await kernel.pipelineDiagnostic(pipeline) {
                problems[pipeline.id] = diagnostic
            }
        }
        signatures = resolved
        issues = problems
    }

    func delete(_ pipeline: Pipeline) {
        let kernel = kernel
        Task {
            await kernel.pipelineStore.delete(id: pipeline.id)
            await self.refresh()
        }
    }
}

@MainActor
struct PipelineComposerDraft {
    var name = ""
    var stages: [PipelineStage] = []
}

struct PipelinesPane: View {
    @Bindable var shell: ShellModel
    @State private var model: PipelinesModel?
    @State private var composing = false
    @State private var editDraft: Pipeline?
    @State private var query = ""

    private static let contentWidth: CGFloat = 1080

    private var records: [ModelRecord] {
        shell.library.records.filter { $0.state == .ready }
    }

    var body: some View {
        Group {
            if let model, let id = shell.pipelineSelection,
                let pipeline = model.pipelines.first(where: { $0.id == id })
            {
                runScreen(model: model, pipeline: pipeline)
            } else {
                dashboard
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        .background(PixelGrid())
        .task {
            if model == nil { model = PipelinesModel(kernel: shell.kernel) }
            await model?.refresh()
        }
        .modalScrim(isPresented: composing, anchor: .top, onDismiss: { composing = false }) {
            PipelineComposerSheet(
                shell: shell, existing: nil,
                onSaved: {
                    composing = false
                    Task { await model?.refresh() }
                }, onClose: { composing = false })
        }
        .modalScrim(isPresented: editDraft != nil, anchor: .top, onDismiss: { editDraft = nil }) {
            if let editDraft {
                PipelineComposerSheet(
                    shell: shell, existing: editDraft,
                    onSaved: {
                        self.editDraft = nil
                        Task { await model?.refresh() }
                    }, onClose: { self.editDraft = nil })
            }
        }
    }

    private var dashboard: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: Design.Space.pane) {
                hero
                content
            }
            .padding(.horizontal, Design.Space.gutter)
            .padding(.top, Design.Space.xxl)
            .padding(.bottom, Design.Space.pane)
            .frame(maxWidth: Self.contentWidth, alignment: .leading)
            .frame(maxWidth: .infinity, alignment: .center)
        }
    }

    private var hero: some View {
        HStack(alignment: .top, spacing: Design.Space.l) {
            VStack(alignment: .leading, spacing: Design.Space.s) {
                Text("Pipelines")
                    .font(Design.hero)
                    .foregroundStyle(Design.ink)
                Text("Chain your models into repeatable flows.")
                    .font(Design.readingBody)
                    .foregroundStyle(Design.inkSoft)
            }
            Spacer(minLength: 0)
            QuietIconButton(glyph: "plus") {
                composing = true
            }
            .accessibilityIdentifier("pipelines-new")
            .help("New pipeline")
            .accessibilityLabel("New pipeline")
        }
    }

    @ViewBuilder
    private var content: some View {
        if let model {
            if model.pipelines.isEmpty {
                emptyState
            } else {
                let rows = filtered(model)
                VStack(alignment: .leading, spacing: Design.Space.m) {
                    if model.pipelines.count > 5 {
                        HStack(spacing: Design.Space.m) {
                            MicroHeader(title: "Saved · \(model.pipelines.count)")
                            Spacer(minLength: 0)
                            InkSearchField(
                                placeholder: "Filter by name", query: $query,
                                fill: Design.surface)
                                .frame(width: 200)
                        }
                    }
                    if rows.isEmpty {
                        ModeEmptyState(
                            glyph: "magnifyingglass",
                            eyebrow: "Filtered view",
                            headline: "Nothing found.",
                            caption: "No saved pipeline matches that name."
                        ) {
                            Button("Clear filter") { query = "" }
                                .buttonStyle(QuietButtonStyle())
                        }
                        .frame(minHeight: 220)
                    } else {
                        grid(model: model, rows: rows)
                    }
                }
            }
        }
    }

    private func grid(model: PipelinesModel, rows: [Pipeline]) -> some View {
        LazyVGrid(
            columns: [
                GridItem(.adaptive(minimum: 300), spacing: Design.Space.l, alignment: .top)
            ],
            spacing: Design.Space.l
        ) {
            ForEach(rows) { pipeline in
                PipelineCard(
                    pipeline: pipeline,
                    signature: model.signatures[pipeline.id],
                    issue: model.issues[pipeline.id],
                    records: records,
                    onRun: { shell.pipelineSelection = pipeline.id },
                    onEdit: { editDraft = pipeline },
                    onDelete: { model.delete(pipeline) })
            }
        }
    }

    private var emptyState: some View {
        ModeEmptyState(
            eyebrow: "Pipelines",
            headline: "No pipelines yet",
            caption:
                "Wire the output of one model into the next — transcribe → chat → speak makes a voice assistant from three models you already have."
        ) {
            Button("New pipeline") { composing = true }
                .buttonStyle(InkButtonStyle())
        }
        .frame(minHeight: 320)
        .surfaceCard(radius: Design.Radius.card)
    }

    private func filtered(_ model: PipelinesModel) -> [Pipeline] {
        let needle = query.trimmingCharacters(in: .whitespaces).lowercased()
        guard !needle.isEmpty else { return model.pipelines }
        return model.pipelines.filter { $0.name.lowercased().contains(needle) }
    }

    @ViewBuilder
    private func runScreen(model: PipelinesModel, pipeline: Pipeline) -> some View {
        if let signature = model.signatures[pipeline.id] {
            PipelineRunScreen(
                kernel: shell.kernel, pipeline: pipeline, signature: signature,
                audio: shell.audio,
                onBack: { shell.pipelineSelection = nil },
                onEdit: { editDraft = pipeline })
                .id(pipeline.id)
        } else {
            VStack(spacing: 0) {
                PipelineRunHeader(
                    name: pipeline.name, stages: pipeline.stages, signature: nil,
                    onBack: { shell.pipelineSelection = nil },
                    onEdit: { editDraft = pipeline })
                ModeEmptyState(
                    headline: "This pipeline can't run right now",
                    caption: (model.issues[pipeline.id] ?? "A stage isn't ready.")
                        + " Edit the pipeline or bring the model back."
                ) {
                    Button("Edit pipeline") { editDraft = pipeline }
                        .buttonStyle(InkButtonStyle())
                }
            }
        }
    }
}

private struct PipelineCard: View {
    let pipeline: Pipeline
    let signature: PipelineSignature?
    let issue: String?
    let records: [ModelRecord]
    let onRun: () -> Void
    let onEdit: () -> Void
    let onDelete: () -> Void
    @State private var hovering = false

    private var modelNames: String {
        let names = pipeline.stages.compactMap { stage in
            records.first { $0.id == stage.modelID }?.displayName
        }
        return names.isEmpty ? "\(pipeline.stages.count) stages" : names.joined(separator: " · ")
    }

    var body: some View {
        Button(action: onRun) {
            VStack(alignment: .leading, spacing: Design.Space.l) {
                HStack(alignment: .center, spacing: Design.Space.m) {
                    IconPlaque(size: 34) {
                        Image(systemName: "point.3.connected.trianglepath.dotted")
                            .font(Design.glyphInline)
                            .foregroundStyle(Design.inkSoft)
                    }
                    VStack(alignment: .leading, spacing: 2) {
                        Text(pipeline.name)
                            .font(Design.title)
                            .tracking(Design.tightTracking)
                            .foregroundStyle(Design.ink)
                            .lineLimit(1)
                            .truncationMode(.tail)
                        Text(modelNames)
                            .font(Design.label)
                            .foregroundStyle(Design.inkFaint)
                            .lineLimit(1)
                            .truncationMode(.tail)
                    }
                    Spacer(minLength: 0)
                }
                PipelineFlow(stages: pipeline.stages, signature: signature)
                Spacer(minLength: 0)
                HStack(spacing: Design.Space.s) {
                    if signature != nil {
                        TintChip(text: "Runnable", glyph: "play")
                    } else {
                        TintChip(text: "Needs attention", glyph: "exclamationmark.triangle", faint: true)
                    }
                    Spacer(minLength: 0)
                    Text("\(pipeline.stages.count) \(pipeline.stages.count == 1 ? "stage" : "stages")")
                        .font(Design.micro)
                        .tracking(Design.microTracking)
                        .foregroundStyle(Design.inkFaint)
                }
            }
            .padding(Design.Space.tile)
            .frame(maxWidth: .infinity, minHeight: 140, alignment: .topLeading)
            .background(Design.surface, in: RoundedRectangle.soft(Design.Radius.tile))
            .overlay(
                RoundedRectangle.soft(Design.Radius.tile)
                    .strokeBorder(
                        hovering ? AnyShapeStyle(Design.accentEdge) : AnyShapeStyle(Design.line),
                        lineWidth: Design.hairlineWidth))
            .contentShape(RoundedRectangle.soft(Design.Radius.tile))
            .lifts(hovering: hovering)
        }
        .buttonStyle(.plain)
        .overlay(alignment: .topTrailing) {
            HStack(spacing: Design.Space.xs) {
                QuietIconButton(glyph: "square.and.pencil", action: onEdit)
                    .help("Edit pipeline")
                    .accessibilityLabel("Edit \(pipeline.name)")
                QuietIconButton(glyph: "trash", action: onDelete)
                    .help("Delete pipeline")
                    .accessibilityLabel("Delete \(pipeline.name)")
            }
            .padding(Design.Space.m)
            .opacity(hovering ? 1 : 0)
            .allowsHitTesting(hovering)
            .animation(Design.wash, value: hovering)
        }
        .onHover { hovering = $0 }
        .animation(Design.wash, value: hovering)
        .contextMenu {
            Button("Run", action: onRun)
            Button("Edit", action: onEdit)
            Button("Delete", role: .destructive, action: onDelete)
        }
        .help("Run \(pipeline.name)")
        .accessibilityIdentifier("pipeline-row")
    }
}

private struct PipelineFlow: View {
    let stages: [PipelineStage]
    let signature: PipelineSignature?

    var body: some View {
        HStack(spacing: Design.Space.xs) {
            if let signature {
                PortChip(port: signature.input)
                connector
            }
            ForEach(Array(stages.enumerated()), id: \.element.id) { index, stage in
                if index > 0 { connector }
                StageChip(capability: stage.capability)
            }
            if let signature {
                connector
                PortChip(port: signature.output)
            }
        }
    }

    private var connector: some View {
        Image(systemName: "chevron.compact.right")
            .font(Design.glyphSmall)
            .foregroundStyle(Design.inkFaint)
    }
}

private struct StageChip: View {
    let capability: Capability

    var body: some View {
        TintChip(text: capability.rawValue.capitalized, glyph: glyph)
    }

    private var glyph: String {
        switch capability {
        case .chat: "message"
        case .complete: "text.alignleft"
        case .transcribe: "waveform"
        case .speak: "speaker.wave.2"
        case .image: "photo"
        case .embed: "circle.grid.3x3"
        case .see: "eye"
        default: "circle"
        }
    }
}

private struct PortChip: View {
    let port: PipelinePort

    var body: some View {
        Text(port.rawValue.capitalized)
            .font(Design.label.weight(.medium))
            .foregroundStyle(Design.inkSoft)
            .padding(.horizontal, Design.Space.s)
            .padding(.vertical, 2)
            .background(
                RoundedRectangle.soft(Design.Radius.control)
                    .strokeBorder(Design.line, lineWidth: Design.hairlineWidth))
    }
}

private struct PipelineRunHeader: View {
    let name: String
    let stages: [PipelineStage]
    let signature: PipelineSignature?
    var running = false
    var status: String? = nil
    let onBack: () -> Void
    let onEdit: () -> Void

    private static let contentWidth: CGFloat = 1080

    var body: some View {
        VStack(spacing: 0) {
            HStack(alignment: .center, spacing: Design.Space.l) {
                QuietIconButton(glyph: "chevron.left", action: onBack)
                    .help("All pipelines")
                    .accessibilityLabel("Back to pipelines")
                VStack(alignment: .leading, spacing: Design.Space.xs) {
                    Text(name)
                        .font(Design.paneTitle)
                        .tracking(Design.tightTracking)
                        .foregroundStyle(Design.ink)
                        .lineLimit(1)
                    PipelineFlow(stages: stages, signature: signature)
                }
                Spacer(minLength: Design.Space.l)
                if running, let status {
                    HStack(spacing: Design.Space.s) {
                        AccentDot(size: 7)
                        Text(status)
                            .font(Design.micro)
                            .tracking(Design.microTracking)
                            .foregroundStyle(Design.accentText)
                            .lineLimit(1)
                    }
                }
                QuietIconButton(glyph: "square.and.pencil", action: onEdit)
                    .help("Edit pipeline")
                    .accessibilityLabel("Edit pipeline")
            }
            .padding(.horizontal, Design.Space.gutter)
            .padding(.top, Design.Space.xxl)
            .padding(.bottom, Design.Space.l)
            .frame(maxWidth: Self.contentWidth, alignment: .leading)
            .frame(maxWidth: .infinity, alignment: .center)
            Rectangle()
                .fill(Design.hairline)
                .frame(height: Design.hairlineWidth)
        }
    }
}

private struct PipelineComposerSheet: View {
    let shell: ShellModel
    let existing: Pipeline?
    let onSaved: () -> Void
    let onClose: () -> Void

    @State private var name = ""
    @State private var stages: [PipelineStage] = []
    @State private var failure: String?
    @State private var saving = false

    private var records: [ModelRecord] {
        shell.library.records.filter { $0.state == .ready }
    }

    private func candidates(_ capability: Capability) -> [ModelRecord] {
        records.filter { $0.capabilities.contains(capability) }
    }

    private var signature: PipelineSignature? {
        try? PipelineValidator.validate(stages, shelf: records)
    }

    private var nextChoices: [Capability] {
        PipelineValidator.nextCapabilities(after: stages)
            .filter { !candidates($0).isEmpty }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
                .padding(.horizontal, Design.Space.gutter)
                .padding(.top, Design.Space.gutter)
                .padding(.bottom, Design.Space.xl)
            Rectangle().fill(Design.hairline).frame(height: Design.hairlineWidth)
            ScrollView {
                VStack(alignment: .leading, spacing: Design.Space.xl) {
                    nameSection
                    stagesSection
                    if let failure {
                        noticeRow(failure)
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
        .onAppear {
            if let existing {
                name = existing.name
                stages = existing.stages
            }
        }
    }

    private var header: some View {
        HStack(alignment: .center, spacing: Design.Space.l) {
            IconPlaque(size: 44) {
                Image(systemName: "point.3.connected.trianglepath.dotted")
                    .font(Design.glyphNav)
                    .foregroundStyle(Design.inkSoft)
            }
            VStack(alignment: .leading, spacing: Design.Space.xxs) {
                Text(existing == nil ? "New pipeline" : "Edit pipeline")
                    .font(Design.title)
                    .tracking(Design.tightTracking)
                Text("Chain models — each stage feeds the next")
                    .font(Design.label)
                    .foregroundStyle(Design.inkFaint)
            }
            Spacer()
            SheetCloseButton(action: onClose)
        }
    }

    private var nameSection: some View {
        VStack(alignment: .leading, spacing: Design.Space.m) {
            MicroHeader(title: "Name")
            InkField(placeholder: "voice reply", text: $name)
                .accessibilityIdentifier("pipeline-name")
        }
    }

    private var stagesSection: some View {
        VStack(alignment: .leading, spacing: Design.Space.m) {
            MicroHeader(title: "Stages")
            VStack(alignment: .leading, spacing: Design.Space.s) {
                ForEach(Array(stages.enumerated()), id: \.element.id) { index, stage in
                    stageRow(index: index, stage: stage)
                }
                if !nextChoices.isEmpty {
                    addStageRow
                }
                if stages.isEmpty {
                    Text("Start with any model — the next stage can only be one whose input matches.")
                        .font(Design.label)
                        .foregroundStyle(Design.inkFaint)
                }
            }
            .padding(Design.Space.tile)
            .surfaceCard(radius: Design.Radius.tile)
            if let signature {
                HStack(spacing: Design.Space.xs) {
                    PortChip(port: signature.input)
                    Text("in")
                        .font(Design.micro).tracking(Design.microTracking)
                        .foregroundStyle(Design.inkFaint)
                    Spacer()
                    Text("out")
                        .font(Design.micro).tracking(Design.microTracking)
                        .foregroundStyle(Design.inkFaint)
                    PortChip(port: signature.output)
                }
            }
        }
    }

    private func stageRow(index: Int, stage: PipelineStage) -> some View {
        HStack(spacing: Design.Space.m) {
            Text("\(index + 1)")
                .font(Design.data(11))
                .foregroundStyle(Design.inkFaint)
                .frame(width: 16)
            Text(stage.capability.rawValue)
                .font(Design.caption.weight(.medium))
                .foregroundStyle(Design.ink)
                .frame(width: 84, alignment: .leading)
            InkDropdown(
                options: candidates(stage.capability).map(\.displayName),
                selection: records.first { $0.id == stage.modelID }?.displayName,
                placeholder: "pick a model",
                allowsAuto: false,
                accessibilityName: "\(stage.capability.rawValue) model"
            ) { picked in
                guard let picked,
                    let record = candidates(stage.capability).first(where: {
                        $0.displayName == picked
                    })
                else { return }
                stages[index].modelID = record.id
            }
            Spacer(minLength: 0)
            Button {
                stages.removeSubrange(index..<stages.count)
            } label: {
                Image(systemName: "xmark.circle.fill")
                    .font(Design.glyphInline)
                    .foregroundStyle(Design.inkFaint)
            }
            .buttonStyle(PressDipStyle())
            .accessibilityLabel("Remove stage \(index + 1)")
        }
    }

    private var addStageRow: some View {
        HStack(spacing: Design.Space.m) {
            Image(systemName: "plus")
                .font(Design.glyphInline)
                .foregroundStyle(Design.inkSoft)
                .frame(width: 16)
            InkDropdown(
                options: nextChoices.map(\.rawValue),
                selection: nil,
                placeholder: "add a stage",
                allowsAuto: false,
                accessibilityName: "add stage"
            ) { picked in
                guard let picked else { return }
                let capability = Capability(rawValue: picked)
                guard let record = candidates(capability).first else { return }
                stages.append(
                    PipelineStage(modelID: record.id, capability: capability))
            }
            Spacer(minLength: 0)
        }
        .accessibilityIdentifier("pipeline-add-stage")
    }

    private func noticeRow(_ text: String) -> some View {
        HStack(spacing: Design.Space.s) {
            Image(systemName: "exclamationmark.triangle")
                .font(Design.glyphInline)
                .foregroundStyle(Design.inkSoft)
            Text(text)
                .font(Design.caption.weight(.medium))
                .foregroundStyle(Design.inkSoft)
        }
    }

    private var footer: some View {
        HStack {
            Spacer()
            Button(saving ? "Saving…" : "Save") {
                save()
            }
            .buttonStyle(InkButtonStyle())
            .keyboardShortcut(.defaultAction)
            .disabled(name.trimmingCharacters(in: .whitespaces).isEmpty || stages.isEmpty || saving)
            .accessibilityIdentifier("pipeline-save")
        }
    }

    private func save() {
        saving = true
        failure = nil
        var pipeline = existing ?? Pipeline(name: name, stages: stages)
        pipeline.name = name.trimmingCharacters(in: .whitespaces)
        pipeline.stages = stages
        let kernel = shell.kernel
        let onSaved = onSaved
        Task { @MainActor in
            do {
                _ = try await kernel.pipelineStore.save(pipeline)
                onSaved()
            } catch let error as PipelineValidationError {
                failure = error.description
            } catch {
                failure = error.localizedDescription
            }
            saving = false
        }
    }
}

struct PipelineTurn: Identifiable {
    let id = UUID()
    let prompt: String
    var response: String = ""
    var spoke = false
    var image: String?
}

@Observable
@MainActor
final class PipelineRunModel {
    let kernel: Kernel
    let pipeline: Pipeline
    let signature: PipelineSignature

    var draft = ""
    var turns: [PipelineTurn] = []
    var status: String?
    var running = false
    var notice: String?
    var listening = false

    let audio: AudioSession
    private let capture = MicCapture()
    private var vad = VADLite()
    private var runTask: Task<Void, Never>?
    private var feedTask: Task<Void, Never>?

    init(kernel: Kernel, pipeline: Pipeline, signature: PipelineSignature, audio: AudioSession) {
        self.kernel = kernel
        self.pipeline = pipeline
        self.signature = signature
        self.audio = audio
    }

    private var trackID: String { "pipeline-\(pipeline.id)" }

    private func beginSpeakingIfNeeded() {
        guard !audio.isActive(trackID) else { return }
        audio.beginLive(
            AudioSession.Track(id: trackID, title: pipeline.name, subtitle: "pipeline"),
            audible: true,
            onStop: { [weak self] in self?.stop() })
    }

    private func endSpeaking() {
        if audio.isActive(trackID) {
            audio.finishLive(trackID)
        }
    }

    var isAudioHead: Bool { signature.input == .audio }

    var canSend: Bool {
        !draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && !running
    }

    func runText() {
        let text = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty, !running else { return }
        draft = ""
        status = nil
        notice = nil
        running = true
        turns.append(PipelineTurn(prompt: text))
        runTask = Task { [weak self] in
            guard let self else { return }
            await self.consume(input: .text(text))
            self.endSpeaking()
            self.running = false
            self.status = nil
        }
    }

    private func consume(input: PipelineInput) async {
        do {
            let stream = try await kernel.runPipeline(pipeline, input: input)
            for await event in stream {
                switch event {
                case .transcript(_, let text):
                    let heard = text.trimmingCharacters(in: .whitespacesAndNewlines)
                    if !heard.isEmpty { turns.append(PipelineTurn(prompt: heard)) }
                case .delta(_, let delta):
                    appendResponse(delta)
                case .audio(let frame):
                    status = "Speaking…"
                    markSpoke()
                    beginSpeakingIfNeeded()
                    audio.enqueue(frame, for: trackID)
                case .artifact(let id):
                    status = nil
                    markImage(id)
                case .status(_, let message):
                    status = message.capitalized + "…"
                case .vector(let values):
                    status = nil
                    appendResponse("embedding · \(values.count) dimensions")
                case .failed(let message):
                    notice = message
                case .cancelled:
                    status = "Cancelled"
                case .stageStarted, .completed:
                    break
                }
            }
        } catch {
            notice = error.localizedDescription
        }
    }

    private func appendResponse(_ delta: String) {
        guard !turns.isEmpty else { return }
        turns[turns.count - 1].response += delta
    }

    private func markSpoke() {
        guard !turns.isEmpty else { return }
        turns[turns.count - 1].spoke = true
    }

    private func markImage(_ id: String) {
        guard !turns.isEmpty else { return }
        turns[turns.count - 1].image = id
    }

    func toggleListening() {
        if listening {
            stop()
            return
        }
        Task { [weak self] in
            let granted = await AVCaptureDevice.requestAccess(for: .audio)
            guard let self else { return }
            guard granted else {
                notice = "Hedos needs microphone access to run this pipeline."
                return
            }
            self.startListening()
        }
    }

    private func startListening() {
        vad = VADLite()
        let (samples, feed) = AsyncStream.makeStream(of: [Float].self)
        do {
            try capture.start { chunk in feed.yield(chunk) }
        } catch {
            notice = error.localizedDescription
            return
        }
        listening = true
        status = "Listening…"
        feedTask = Task { [weak self] in
            for await chunk in samples {
                guard let self else { return }
                for event in self.vad.consume(chunk) {
                    switch event {
                    case .speechStarted:
                        self.audio.flushLive(self.trackID)
                        self.runTask?.cancel()
                        self.status = "Hearing you…"
                    case .turnEnded(let turn):
                        self.startTurn(turn)
                    }
                }
            }
        }
    }

    private func startTurn(_ samples: [Float]) {
        runTask?.cancel()
        runTask = Task { [weak self] in
            await self?.consume(input: .audio(samples))
        }
    }

    func stop() {
        listening = false
        status = nil
        capture.stop()
        endSpeaking()
        runTask?.cancel()
        feedTask?.cancel()
        runTask = nil
        feedTask = nil
    }
}

private struct PipelineRunScreen: View {
    @State private var model: PipelineRunModel
    let onBack: () -> Void
    let onEdit: () -> Void

    init(
        kernel: Kernel, pipeline: Pipeline, signature: PipelineSignature,
        audio: AudioSession, onBack: @escaping () -> Void, onEdit: @escaping () -> Void
    ) {
        _model = State(
            initialValue: PipelineRunModel(
                kernel: kernel, pipeline: pipeline, signature: signature, audio: audio))
        self.onBack = onBack
        self.onEdit = onEdit
    }

    var body: some View {
        VStack(spacing: 0) {
            PipelineRunHeader(
                name: model.pipeline.name,
                stages: model.pipeline.stages,
                signature: model.signature,
                running: model.running || model.listening,
                status: model.status,
                onBack: onBack,
                onEdit: onEdit)
            PipelineRunContent(model: model)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .onDisappear { model.stop() }
    }
}

private struct PipelineRunContent: View {
    @Bindable var model: PipelineRunModel
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        if model.isAudioHead {
            audioSurface
        } else {
            ConversationScaffold(
                placeholder: "Send a prompt through \(model.pipeline.name)",
                draft: $model.draft,
                isWorking: model.running,
                canSend: model.canSend,
                notice: model.notice,
                onSend: { model.runText() },
                onStop: { model.stop() },
                transcript: { transcript },
                header: {},
                aux: {},
                chip: {}
            )
        }
    }

    private var transcript: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: Design.Space.xl) {
                    if model.turns.isEmpty {
                        ModeEmptyState(
                            eyebrow: model.pipeline.name,
                            headline: "Run the chain",
                            caption:
                                "Send a prompt — it flows through every stage and the last one's output comes back."
                        )
                        .frame(minHeight: Design.Column.hero)
                    }
                    ForEach(model.turns) { turn in
                        turnView(turn)
                            .id(turn.id)
                    }
                    if let status = model.status {
                        Text(status)
                            .font(Design.micro).tracking(Design.microTracking)
                            .foregroundStyle(Design.accent)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .id("status")
                    }
                }
                .padding(.horizontal, Design.Space.xxl)
                .padding(.vertical, Design.Space.xl)
                .frame(maxWidth: Design.Column.transcriptProse, alignment: .leading)
                .frame(maxWidth: .infinity)
            }
            .onChange(of: model.turns.last?.response) {
                guard let last = model.turns.last else { return }
                if model.running {
                    proxy.scrollTo(last.id, anchor: .bottom)
                } else {
                    withAnimation(Design.motion(reduceMotion: reduceMotion)) {
                        proxy.scrollTo(last.id, anchor: .bottom)
                    }
                }
            }
            .onChange(of: model.turns.count) {
                if let last = model.turns.last {
                    withAnimation(Design.motion(reduceMotion: reduceMotion)) {
                        proxy.scrollTo(last.id, anchor: .bottom)
                    }
                }
            }
        }
    }

    @ViewBuilder
    private func turnView(_ turn: PipelineTurn) -> some View {
        VStack(alignment: .leading, spacing: Design.Space.m) {
            PromptBubble(text: turn.prompt)
            if !turn.response.isEmpty {
                Text(turn.response)
                    .font(Design.body)
                    .foregroundStyle(Design.ink)
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .responseShell()
            }
            if let image = turn.image {
                ArtifactExchangeView(
                    reference: image, kernel: model.kernel, session: model.audio)
            }
            if turn.spoke {
                HStack(spacing: Design.Space.xs) {
                    Image(systemName: "speaker.wave.2.fill")
                        .font(Design.glyphInline)
                        .foregroundStyle(Design.accent)
                    Text("spoken")
                        .font(Design.micro).tracking(Design.microTracking)
                        .foregroundStyle(Design.inkSoft)
                }
            }
        }
    }

    private var audioSurface: some View {
        VStack(spacing: Design.Space.l) {
            if model.turns.isEmpty {
                ModeEmptyState(
                    eyebrow: model.pipeline.name,
                    headline: "Talk to run it",
                    caption: "Tap the mic and speak — your voice runs through every stage.")
            } else {
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: Design.Space.xl) {
                        ForEach(model.turns) { turn in
                            turnView(turn)
                        }
                    }
                    .padding(Design.Space.xxl)
                    .frame(maxWidth: Design.Column.transcriptProse, alignment: .leading)
                    .frame(maxWidth: .infinity)
                }
            }
            if let status = model.status {
                Text(status)
                    .font(Design.micro).tracking(Design.microTracking)
                    .foregroundStyle(Design.accent)
            }
            if let notice = model.notice {
                Text(notice)
                    .font(Design.label)
                    .foregroundStyle(Design.inkSoft)
            }
            Button {
                model.toggleListening()
            } label: {
                Image(systemName: model.listening ? "stop.fill" : "mic.fill")
                    .font(Design.glyphNav)
                    .foregroundStyle(model.listening ? Design.accent : Design.inkSoft)
                    .frame(width: 52, height: 52)
                    .background(RoundedRectangle.soft(Design.Radius.control).fill(Design.surface))
                    .overlay(RoundedRectangle.soft(Design.Radius.control).strokeBorder(Design.line, lineWidth: Design.hairlineWidth))
            }
            .buttonStyle(PressDipStyle())
            .accessibilityIdentifier("pipeline-mic")
            .padding(.bottom, Design.Space.xl)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}
