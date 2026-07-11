@preconcurrency import AVFoundation
import HedosKernel
import SwiftUI

@Observable
@MainActor
final class PipelinesModel {
    let kernel: Kernel
    var pipelines: [Pipeline] = []
    var signatures: [String: PipelineSignature] = [:]

    init(kernel: Kernel) {
        self.kernel = kernel
    }

    func refresh() async {
        pipelines = await kernel.pipelineStore.list()
        var resolved: [String: PipelineSignature] = [:]
        for pipeline in pipelines {
            if let signature = await kernel.pipelineStore.signature(of: pipeline) {
                resolved[pipeline.id] = signature
            }
        }
        signatures = resolved
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

    private var records: [ModelRecord] {
        shell.library.records.filter { $0.state == .ready }
    }

    var body: some View {
        HStack(spacing: 0) {
            pipelinesColumn
                .frame(width: Design.Rail.columnWidth)
            ColumnDivider()
            runPanel
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        .task {
            if model == nil { model = PipelinesModel(kernel: shell.kernel) }
            await model?.refresh()
        }
        .modalScrim(isPresented: composing, onDismiss: { composing = false }) {
            PipelineComposerSheet(
                shell: shell, existing: nil,
                onSaved: {
                    composing = false
                    Task { await model?.refresh() }
                }, onClose: { composing = false })
        }
        .modalScrim(isPresented: editDraft != nil, onDismiss: { editDraft = nil }) {
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

    private var pipelinesColumn: some View {
        VStack(spacing: 0) {
            HStack(spacing: Design.Space.s) {
                InkSearchField(
                    placeholder: "Search pipelines", query: $query, fill: Design.surface)
                QuietIconButton(glyph: "plus") {
                    composing = true
                }
                .accessibilityIdentifier("pipelines-new")
                .help("New pipeline")
                .accessibilityLabel("New pipeline")
            }
            .padding(.horizontal, Design.Space.m)
            .padding(.top, Design.Space.xxl)
            .padding(.bottom, Design.Space.s)
            pipelineList
        }
    }

    private func filtered(_ model: PipelinesModel) -> [Pipeline] {
        let needle = query.trimmingCharacters(in: .whitespaces).lowercased()
        guard !needle.isEmpty else { return model.pipelines }
        return model.pipelines.filter { $0.name.lowercased().contains(needle) }
    }

    @ViewBuilder
    private var pipelineList: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: Design.Space.xxs) {
                if let model {
                    let rows = filtered(model)
                    if rows.isEmpty {
                        Text(
                            model.pipelines.isEmpty
                                ? "No pipelines yet." : "Nothing found."
                        )
                        .font(Design.caption)
                        .foregroundStyle(Design.inkFaint)
                        .padding(Design.Space.m)
                    } else {
                        ForEach(rows) { pipeline in
                            PipelineRow(
                                pipeline: pipeline,
                                signature: model.signatures[pipeline.id],
                                selected: shell.pipelineSelection == pipeline.id,
                                onSelect: { shell.pipelineSelection = pipeline.id },
                                onEdit: { editDraft = pipeline },
                                onDelete: { model.delete(pipeline) })
                        }
                    }
                }
            }
            .padding(.horizontal, Design.Space.m)
            .padding(.top, Design.Space.s)
            .padding(.bottom, Design.Space.l)
        }
    }

    @ViewBuilder
    private var runPanel: some View {
        if let model, model.pipelines.isEmpty {
            ModeEmptyState(
                eyebrow: "Pipelines",
                headline: "Chain a few models into one tool",
                caption:
                    "Wire the output of one model into the next — transcribe → chat → speak makes a voice assistant from three models you already have."
            ) {
                Button("New pipeline") { composing = true }
                    .buttonStyle(InkButtonStyle())
            }
        } else if let model, let id = shell.pipelineSelection,
            let pipeline = model.pipelines.first(where: { $0.id == id }),
            let signature = model.signatures[id]
        {
            PipelineRunView(
                kernel: shell.kernel, pipeline: pipeline, signature: signature,
                audio: shell.audio)
                .id(pipeline.id)
        } else {
            ModeEmptyState(
                headline: "Pick a pipeline",
                caption: "Select one on the left to run it, or make a new one.")
        }
    }
}

private struct PipelineRow: View {
    let pipeline: Pipeline
    let signature: PipelineSignature?
    let selected: Bool
    let onSelect: () -> Void
    let onEdit: () -> Void
    let onDelete: () -> Void
    @State private var hovering = false

    var body: some View {
        Button(action: onSelect) {
            VStack(alignment: .leading, spacing: Design.Space.xs) {
                Text(pipeline.name)
                    .font(Design.caption.weight(.medium))
                    .foregroundStyle(Design.ink)
                    .lineLimit(1)
                HStack(spacing: Design.Space.xxs) {
                    if let signature {
                        PortChip(port: signature.input)
                        Image(systemName: "arrow.right")
                            .font(.system(size: 8))
                            .foregroundStyle(Design.inkFaint)
                    }
                    ForEach(Array(pipeline.stages.enumerated()), id: \.offset) { _, stage in
                        Text(stage.capability.rawValue)
                            .font(Design.micro)
                            .tracking(Design.microTracking)
                            .foregroundStyle(Design.inkSoft)
                    }
                    if let signature {
                        Image(systemName: "arrow.right")
                            .font(.system(size: 8))
                            .foregroundStyle(Design.inkFaint)
                        PortChip(port: signature.output)
                    }
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(Design.Space.m)
            .background(
                RoundedRectangle(cornerRadius: Design.Radius.card)
                    .fill(
                        selected
                            ? Design.ink.opacity(0.08)
                            : hovering ? Design.ink.opacity(0.02) : .clear))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
        .contextMenu {
            Button("Edit", action: onEdit)
            Button("Delete", role: .destructive, action: onDelete)
        }
        .accessibilityIdentifier("pipeline-row")
    }
}

private struct PortChip: View {
    let port: PipelinePort

    var body: some View {
        Text(label.uppercased())
            .font(Design.micro)
            .tracking(Design.microTracking)
            .foregroundStyle(Design.inkSoft)
            .padding(.horizontal, Design.Space.xs)
            .padding(.vertical, 1)
            .background(
                RoundedRectangle(cornerRadius: Design.Radius.control).strokeBorder(Design.line, lineWidth: Design.hairlineWidth))
    }

    private var label: String {
        switch port {
        case .audio: "audio"
        case .text: "text"
        case .image: "image"
        case .vector: "vector"
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

    private let audio: AudioSession
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

private struct PipelineRunView: View {
    @State private var model: PipelineRunModel

    init(kernel: Kernel, pipeline: Pipeline, signature: PipelineSignature, audio: AudioSession) {
        _model = State(
            initialValue: PipelineRunModel(
                kernel: kernel, pipeline: pipeline, signature: signature, audio: audio))
    }

    var body: some View {
        PipelineRunContent(model: model)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .onDisappear { model.stop() }
    }
}

private struct PipelineRunContent: View {
    @Bindable var model: PipelineRunModel

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
                if let last = model.turns.last {
                    withAnimation { proxy.scrollTo(last.id, anchor: .bottom) }
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
                    .background(RoundedRectangle(cornerRadius: Design.Radius.control).fill(Design.surface))
                    .overlay(RoundedRectangle(cornerRadius: Design.Radius.control).strokeBorder(Design.line, lineWidth: Design.hairlineWidth))
            }
            .buttonStyle(PressDipStyle())
            .accessibilityIdentifier("pipeline-mic")
            .padding(.bottom, Design.Space.xl)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}
