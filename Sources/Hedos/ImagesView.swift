import AppKit
import HedosKernel
import ImageIO
import SwiftUI

@Observable
@MainActor
final class ImagesViewModel {
    enum Phase: Equatable {
        case idle
        case queued(String?)
        case preparing
        case running
        case failed(String)
    }

    private let kernel: Kernel
    private var thumbnails: [String: NSImage] = [:]
    private var fullImage: (id: String, image: NSImage)?
    private var watchTask: Task<Void, Never>?
    private var cancelRequested = false
    private var submittedPayload: JSONValue?
    private var reattached = false

    var artifacts: [Artifact] = []
    var prompt = ""
    var form = ParamForm(schema: [])
    var boundModelID: String?
    var phase: Phase = .idle
    var status: String?
    var progress: JobProgress = .none
    var preview: NSImage?
    var jobID: String?
    var notice: String?
    var onLanded: ((String) -> Void)?

    init(kernel: Kernel) {
        self.kernel = kernel
    }

    var isBusy: Bool {
        switch phase {
        case .queued, .preparing, .running: true
        case .idle, .failed: false
        }
    }

    var activeDescription: String? {
        submittedPayload.flatMap(Provenance.prompt(of:))
    }

    var arranged: [Artifact] {
        Gallery.arrange(artifacts, modelID: nil, sort: .newestFirst)
    }

    func artifact(id: String?) -> Artifact? {
        guard let id else { return nil }
        return artifacts.first { $0.id == id }
    }

    func start(records: [ModelRecord]) async {
        await load()
        if boundModelID == nil || records.first(where: { $0.id == boundModelID }) == nil {
            if let record = runnableModels(in: records).first {
                bind(to: record)
            }
        }
        if !reattached {
            reattached = true
            await reattach()
        }
    }

    func load() async {
        let all = (try? await kernel.artifacts()) ?? []
        artifacts = all.filter { $0.capability == .image }
    }

    func runnableModels(in records: [ModelRecord]) -> [ModelRecord] {
        Launcher.models(in: records, for: .images).filter {
            Launcher.destination(for: $0) == .images
        }
    }

    func waitingModels(in records: [ModelRecord]) -> [ModelRecord] {
        Launcher.models(in: records, for: .images).filter {
            Launcher.destination(for: $0) != .images
        }
    }

    func bind(to record: ModelRecord) {
        guard boundModelID != record.id else { return }
        boundModelID = record.id
        form = ParamForm(schema: record.params)
    }

    func adoptParams(from artifact: Artifact, records: [ModelRecord]) {
        guard let record = records.first(where: { $0.id == artifact.modelID }),
            Launcher.destination(for: record) == .images
        else { return }
        boundModelID = record.id
        var fresh = ParamForm(schema: record.params)
        fresh.load(artifact.params)
        form = fresh
        if let text = Provenance.prompt(of: artifact.params) {
            prompt = text
        }
    }

    func generate() {
        let text = prompt.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty, !isBusy, let modelID = boundModelID else { return }
        let kernel = kernel
        let payload = form.payload(prompt: text)
        start(intending: payload) {
            try await kernel.submit(modelID, .image, payload: payload)
        }
    }

    func rerun(_ artifact: Artifact) {
        guard !isBusy else { return }
        let kernel = kernel
        start(intending: artifact.params) {
            try await kernel.rerun(artifactID: artifact.id)
        }
    }

    func vary(_ artifact: Artifact) {
        guard !isBusy else { return }
        let kernel = kernel
        start(intending: artifact.params) {
            try await kernel.vary(artifactID: artifact.id)
        }
    }

    func cancel() {
        guard isBusy else { return }
        cancelRequested = true
        guard let jobID else { return }
        let kernel = kernel
        Task {
            await kernel.cancel(jobID: jobID)
        }
    }

    func copyFailureDetails() {
        guard case .failed(let message) = phase else { return }
        let details = Provenance.failureDetails(
            model: boundModelID ?? "",
            error: message,
            jobID: jobID,
            params: submittedPayload ?? form.payload(prompt: prompt),
            schema: form.schema)
        let pasteboard = NSPasteboard.general
        pasteboard.clearContents()
        pasteboard.setString(details, forType: .string)
    }

    func thumbnail(_ artifact: Artifact) -> NSImage? {
        thumbnails[artifact.id]
    }

    func loadThumbnail(_ artifact: Artifact) async {
        guard thumbnails[artifact.id] == nil else { return }
        if let data = try? await kernel.artifactPreview(id: artifact.id),
            let image = NSImage(data: data)
        {
            thumbnails[artifact.id] = image
            return
        }
        guard let url = try? await kernel.artifactURL(id: artifact.id),
            let image = Self.downsampled(url, maxPixel: 480)
        else { return }
        thumbnails[artifact.id] = image
    }

    func image(for artifact: Artifact) -> NSImage? {
        guard let fullImage, fullImage.id == artifact.id else { return nil }
        return fullImage.image
    }

    func loadImage(_ artifact: Artifact) async {
        guard fullImage?.id != artifact.id else { return }
        guard let url = try? await kernel.artifactURL(id: artifact.id),
            let image = NSImage(contentsOf: url)
        else { return }
        fullImage = (artifact.id, image)
    }

    func delete(_ artifact: Artifact) async {
        do {
            try await kernel.deleteArtifact(id: artifact.id)
            thumbnails[artifact.id] = nil
            if fullImage?.id == artifact.id {
                fullImage = nil
            }
            await load()
        } catch {
            notice = error.localizedDescription
        }
    }

    func download(_ artifact: Artifact) {
        let kernel = kernel
        Task { @MainActor in
            guard let url = try? await kernel.artifactURL(id: artifact.id) else { return }
            let panel = NSSavePanel()
            panel.nameFieldStringValue = url.lastPathComponent
            panel.begin { response in
                guard response == .OK, let destination = panel.url else { return }
                try? FileManager.default.removeItem(at: destination)
                try? FileManager.default.copyItem(at: url, to: destination)
            }
        }
    }

    private func reattach() async {
        let jobs = await kernel.activeJobs()
        guard let job = jobs.first(where: { $0.capability == .image }) else { return }
        jobID = job.id
        submittedPayload = job.payload
        cancelRequested = false
        phase = .queued(job.queueReason)
        if let text = Provenance.prompt(of: job.payload) {
            prompt = text
        }
        form.load(job.payload)
        let id = job.id
        watchTask = Task { [weak self] in
            await self?.watch(id)
        }
    }

    private func start(
        intending payload: JSONValue, _ submit: @escaping @Sendable () async throws -> String
    ) {
        jobID = nil
        submittedPayload = payload
        cancelRequested = false
        phase = .queued(nil)
        status = nil
        progress = .none
        preview = nil
        watchTask?.cancel()
        watchTask = Task { [weak self] in
            guard let self else { return }
            do {
                let id = try await submit()
                jobID = id
                if let job = try? await kernel.job(id: id) {
                    submittedPayload = job.payload
                }
                if cancelRequested {
                    await kernel.cancel(jobID: id)
                }
                await watch(id)
            } catch {
                phase = cancelRequested ? .idle : .failed(error.localizedDescription)
            }
        }
    }

    private func watch(_ id: String) async {
        for await event in await kernel.jobEvents(id: id) {
            switch event {
            case .queued(let reason):
                phase = .queued(reason)
            case .preparing:
                phase = .preparing
            case .status(let message):
                status = message
            case .running:
                phase = .running
                status = nil
            case .progress(let updated):
                progress = updated
            case .preview(let frame):
                preview = NSImage(data: frame)
            case .done(let result):
                await land(result)
            case .failed(let message):
                phase = .failed(message)
            case .cancelled:
                phase = .idle
            }
        }
        guard !Task.isCancelled else { return }
        if isBusy {
            phase = .idle
        }
        status = nil
        preview = nil
        progress = .none
    }

    private func land(_ result: [String]) async {
        await load()
        phase = .idle
        if let landed = result.first {
            onLanded?(landed)
        }
    }

    private static func downsampled(_ url: URL, maxPixel: CGFloat) -> NSImage? {
        let sourceOptions = [kCGImageSourceShouldCache: false] as CFDictionary
        guard let source = CGImageSourceCreateWithURL(url as CFURL, sourceOptions) else {
            return nil
        }
        let thumbnailOptions =
            [
                kCGImageSourceCreateThumbnailFromImageAlways: true,
                kCGImageSourceShouldCacheImmediately: true,
                kCGImageSourceCreateThumbnailWithTransform: true,
                kCGImageSourceThumbnailMaxPixelSize: maxPixel,
            ] as [CFString: Any] as CFDictionary
        guard let cgImage = CGImageSourceCreateThumbnailAtIndex(source, 0, thumbnailOptions)
        else { return nil }
        return NSImage(cgImage: cgImage, size: NSSize(width: cgImage.width, height: cgImage.height))
    }
}

struct ImagesSurface: View {
    @Bindable var shell: ShellModel
    @State private var showParams = false
    @State private var confirmingDelete: Artifact?
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    private var model: ImagesViewModel { shell.images }
    private var boundRecord: ModelRecord? {
        shell.library.record(id: model.boundModelID)
    }

    var body: some View {
        ConversationScaffold(
            placeholder: "What should this look like?",
            draft: Bindable(shell.images).prompt,
            isWorking: model.isBusy,
            canSend: generatable,
            notice: model.notice,
            onSend: { model.generate() },
            onStop: { model.cancel() },
            transcript: { transcript },
            aux: { paramsControl },
            chip: { modelChip }
        )
        .task(id: shell.library.records.count) {
            model.onLanded = { [weak shell] id in
                shell?.selectImages(id)
            }
            await model.start(records: shell.library.records)
        }
        .confirmationDialog(
            "Move this image to the Trash?",
            isPresented: Binding(
                get: { confirmingDelete != nil },
                set: { if !$0 { confirmingDelete = nil } })
        ) {
            Button("Move to Trash", role: .destructive) {
                if let artifact = confirmingDelete {
                    let shell = shell
                    Task {
                        await shell.images.delete(artifact)
                        if shell.imagesSelection == artifact.id {
                            shell.selectImages(nil)
                        }
                    }
                }
                confirmingDelete = nil
            }
        } message: {
            Text("The file moves to the Trash — it is not deleted outright.")
        }
    }

    private var chronological: [Artifact] {
        model.arranged.reversed()
    }

    private var transcript: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: Design.Space.xxl) {
                    if chronological.isEmpty && !model.isBusy {
                        emptyTranscript
                    }
                    ForEach(chronological) { artifact in
                        generationRow(artifact)
                            .id(artifact.id)
                    }
                    if model.isBusy {
                        liveRow
                    }
                    if case .failed(let message) = model.phase {
                        failedRow(message)
                    }
                    Color.clear.frame(height: 1).id("images-tail")
                }
                .padding(.horizontal, Design.Space.xxl)
                .padding(.vertical, Design.Space.xxl)
                .frame(maxWidth: Design.conversationMaxWidth, alignment: .leading)
                .frame(maxWidth: .infinity)
            }
            .onChange(of: model.arranged.count) {
                proxy.scrollTo("images-tail", anchor: .bottom)
            }
            .onChange(of: model.isBusy) {
                if model.isBusy {
                    proxy.scrollTo("images-tail", anchor: .bottom)
                }
            }
            .onChange(of: shell.imagesSelection) { _, selection in
                if let selection, model.artifact(id: selection) != nil {
                    withAnimation(Design.motion(reduceMotion: reduceMotion)) {
                        proxy.scrollTo(selection, anchor: .center)
                    }
                    if let artifact = model.artifact(id: selection) {
                        model.adoptParams(from: artifact, records: shell.library.records)
                    }
                }
            }
        }
    }

    private func generationRow(_ artifact: Artifact) -> some View {
        VStack(alignment: .leading, spacing: Design.Space.m) {
            PromptBubble(text: Provenance.prompt(of: artifact.params) ?? "Untitled")
            ImageBubble(
                image: model.thumbnail(artifact),
                caption: Provenance.line(for: artifact, schema: schema(for: artifact)),
                isLoading: model.thumbnail(artifact) == nil
            )
            .task(id: artifact.id) {
                await model.loadThumbnail(artifact)
            }
            .contextMenu {
                Button("Re-run") {
                    model.rerun(artifact)
                }
                .disabled(model.isBusy || !canRun(artifact))
                Button("Vary") {
                    model.vary(artifact)
                }
                .disabled(model.isBusy || !canRun(artifact))
                Divider()
                Button("Download…") {
                    model.download(artifact)
                }
                Divider()
                Button("Delete…", role: .destructive) {
                    confirmingDelete = artifact
                }
            }
            .overlay(
                RoundedRectangle(cornerRadius: Design.Radius.bubble)
                    .strokeBorder(
                        shell.imagesSelection == artifact.id
                            ? AnyShapeStyle(Design.inkFaint) : AnyShapeStyle(.clear),
                        lineWidth: Design.hairlineWidth))
            .onTapGesture {
                shell.selectImages(artifact.id)
            }
        }
    }

    private var liveRow: some View {
        VStack(alignment: .leading, spacing: Design.Space.m) {
            if let prompt = model.activeDescription {
                PromptBubble(text: prompt)
            }
            VStack(alignment: .leading, spacing: Design.Space.m) {
                ImageBubble(image: model.preview, caption: nil, isLoading: true)
                HStack(spacing: Design.Space.l) {
                    ProgressView(value: model.progress.fraction)
                        .progressViewStyle(.linear)
                        .controlSize(.small)
                        .frame(maxWidth: 220)
                    if let step = model.progress.step, let total = model.progress.totalSteps {
                        Text("step \(step) / \(total)")
                            .font(Design.data(10))
                            .foregroundStyle(Design.inkSoft)
                            .monospacedDigit()
                            .contentTransition(.numericText())
                            .animation(
                                Design.motion(reduceMotion: reduceMotion), value: step)
                    } else if let status = statusLine {
                        Text(status)
                            .font(Design.label)
                            .foregroundStyle(Design.inkFaint)
                    }
                    Button("Cancel") {
                        model.cancel()
                    }
                    .buttonStyle(QuietButtonStyle())
                    .keyboardShortcut(.cancelAction)
                }
            }
        }
    }

    private var statusLine: String? {
        switch model.phase {
        case .queued(let reason): reason ?? model.status ?? "Waiting to run"
        case .preparing: "Preparing image runtime — first use only"
        default: model.status
        }
    }

    private func failedRow(_ message: String) -> some View {
        VStack(alignment: .leading, spacing: Design.Space.m) {
            Label {
                Text(message)
                    .font(Design.caption)
                    .foregroundStyle(Design.inkSoft)
                    .textSelection(.enabled)
            } icon: {
                Image(systemName: "exclamationmark.triangle")
                    .font(Design.glyphInline.weight(.semibold))
                    .foregroundStyle(Design.inkSoft)
            }
            Button("Copy details") {
                model.copyFailureDetails()
            }
            .buttonStyle(QuietButtonStyle())
        }
        .responseShell()
    }

    private var emptyTranscript: some View {
        Text(emptyCaption)
            .font(Design.caption)
            .foregroundStyle(Design.inkFaint)
            .frame(maxWidth: .infinity)
            .padding(.top, 100)
    }

    private var emptyCaption: String {
        if model.runnableModels(in: shell.library.records).isEmpty {
            return "When an image model lands on your shelf, its canvas opens here."
        }
        return "A sentence in, an image out — describe one below."
    }

    private func canRun(_ artifact: Artifact) -> Bool {
        guard let record = shell.library.record(id: artifact.modelID) else { return false }
        return Launcher.destination(for: record) == .images
    }

    private func schema(for artifact: Artifact) -> [ParamSpec] {
        shell.library.record(id: artifact.modelID)?.params ?? []
    }

    private var generatable: Bool {
        !model.prompt.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            && model.boundModelID != nil
    }

    @ViewBuilder
    private var paramsControl: some View {
        if !model.form.schema.isEmpty {
            CircleControl(glyph: "slider.horizontal.3", label: "Generation parameters") {
                showParams.toggle()
            }
            .popover(isPresented: $showParams, arrowEdge: .top) {
                ImageParamsForm(model: model)
            }
        }
    }

    private var modelChip: some View {
        InkMenu(
            title: boundRecord?.displayName ?? "Choose model",
            accessibilityName: "Image model"
        ) {
            let runnable = model.runnableModels(in: shell.library.records)
            let waiting = model.waitingModels(in: shell.library.records)
            if runnable.isEmpty && waiting.isEmpty {
                InkMenuRow(title: "No image model is ready.", disabled: true) {}
            }
            ForEach(runnable) { record in
                InkMenuRow(
                    title: record.displayName,
                    selected: record.id == model.boundModelID
                ) {
                    model.bind(to: record)
                }
            }
            if !waiting.isEmpty {
                InkMenuDivider()
                ForEach(waiting) { record in
                    InkMenuRow(
                        title: record.displayName,
                        annotation: "needs recipe",
                        disabled: true
                    ) {}
                }
            }
        }
        .disabled(model.isBusy)
    }
}

struct ImageParamsForm: View {
    let model: ImagesViewModel

    var body: some View {
        VStack(alignment: .leading, spacing: Design.Space.l) {
            ForEach(model.form.schema, id: \.key) { spec in
                VStack(alignment: .leading, spacing: Design.Space.xs) {
                    Text(spec.key.uppercased())
                        .font(Design.micro)
                        .tracking(Design.microTracking)
                        .foregroundStyle(Design.inkFaint)
                    formControl(spec)
                }
            }
        }
        .padding(Design.Space.xl)
        .frame(width: 260)
        .disabled(model.isBusy)
    }

    private func formControl(_ spec: ParamSpec) -> ParamControl {
        var roll: (() -> Void)?
        if spec.type == .int && spec.intRange == nil {
            roll = { model.form.roll(spec.key) }
        }
        return ParamControl(
            spec: spec,
            get: { model.form.value(spec.key) },
            set: { value in
                if let value {
                    model.form.set(spec.key, to: value)
                } else {
                    model.form.clear(spec.key)
                }
            },
            roll: roll)
    }
}
