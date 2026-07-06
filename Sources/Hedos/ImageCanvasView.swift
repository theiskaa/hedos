import AppKit
import HedosKernel
import SwiftUI

@Observable
@MainActor
final class ImageCanvasViewModel {
    enum Phase: Equatable {
        case idle
        case queued(String?)
        case preparing
        case running
        case failed(String)
        case result(Artifact)
    }

    private let kernel: Kernel
    private let modelID: String
    private let modelName: String
    private var watchTask: Task<Void, Never>?
    private var cancelRequested = false
    private var submittedPayload: JSONValue?

    var prompt = ""
    var form: ParamForm
    var phase: Phase = .idle
    var status: String?
    var progress: JobProgress = .none
    var preview: NSImage?
    var resultImage: NSImage?
    var resultURL: URL?
    var jobID: String?

    init(kernel: Kernel, record: ModelRecord) {
        self.kernel = kernel
        self.modelID = record.id
        self.modelName = record.name
        self.form = ParamForm(schema: record.params)
        reattach()
    }

    var activePrompt: String? {
        submittedPayload.flatMap(Provenance.prompt(of:))
    }

    var isBusy: Bool {
        switch phase {
        case .queued, .preparing, .running: true
        case .idle, .failed, .result: false
        }
    }

    var lastResult: Artifact? {
        if case .result(let artifact) = phase { return artifact }
        return nil
    }

    func generate() {
        let text = prompt.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty, !isBusy else { return }
        let kernel = kernel
        let modelID = modelID
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

    func reveal() {
        guard let resultURL else { return }
        NSWorkspace.shared.activateFileViewerSelecting([resultURL])
    }

    func copyFailureDetails() {
        guard case .failed(let message) = phase else { return }
        let details = Provenance.failureDetails(
            model: modelName,
            error: message,
            jobID: jobID,
            params: submittedPayload ?? form.payload(prompt: prompt),
            schema: form.schema)
        let pasteboard = NSPasteboard.general
        pasteboard.clearContents()
        pasteboard.setString(details, forType: .string)
    }

    private func reattach() {
        let kernel = kernel
        let modelID = modelID
        watchTask = Task { [weak self] in
            let jobs = await kernel.activeJobs()
            guard !Task.isCancelled, let self else { return }
            guard let job = jobs.first(where: { $0.modelID == modelID && $0.capability == .image })
            else { return }
            adopt(job)
            await watch(job.id, fallback: nil)
        }
    }

    private func adopt(_ job: Job) {
        jobID = job.id
        submittedPayload = job.payload
        cancelRequested = false
        phase = .queued(job.queueReason)
        if let text = Provenance.prompt(of: job.payload) {
            prompt = text
        }
        form.load(job.payload)
    }

    private func start(
        intending payload: JSONValue, _ submit: @escaping @Sendable () async throws -> String
    ) {
        let fallback = lastResult
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
                await watch(id, fallback: fallback)
            } catch {
                if cancelRequested {
                    restore(fallback)
                } else {
                    phase = .failed(error.localizedDescription)
                }
            }
        }
    }

    private func watch(_ id: String, fallback: Artifact?) async {
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
                restore(fallback)
            }
        }
        guard !Task.isCancelled else { return }
        if isBusy {
            restore(fallback)
        }
        status = nil
        preview = nil
        progress = .none
    }

    private func land(_ result: [String]) async {
        guard let artifactID = result.first,
            let artifact = try? await kernel.artifact(id: artifactID),
            let url = try? await kernel.artifactURL(id: artifactID),
            let image = NSImage(contentsOf: url)
        else {
            phase = .failed("The job finished but its artifact could not be loaded.")
            return
        }
        resultURL = url
        resultImage = image
        form.load(artifact.params)
        phase = .result(artifact)
    }

    private func restore(_ fallback: Artifact?) {
        if let fallback, resultImage != nil {
            phase = .result(fallback)
        } else {
            phase = .idle
        }
    }
}

struct ImageCanvasView: View {
    let record: ModelRecord
    @State private var model: ImageCanvasViewModel

    init(record: ModelRecord, kernel: Kernel) {
        self.record = record
        _model = State(initialValue: ImageCanvasViewModel(kernel: kernel, record: record))
    }

    var body: some View {
        VStack(spacing: 0) {
            canvas
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            controls
        }
        .navigationTitle(record.name)
        .navigationSubtitle(record.runtime.id ?? "")
    }

    @ViewBuilder
    private var canvas: some View {
        switch model.phase {
        case .idle:
            emptyCanvas
        case .queued(let reason):
            waitingCanvas(
                headline: "Waiting to run",
                detail: reason ?? model.status)
        case .preparing:
            waitingCanvas(
                headline: "Preparing image runtime — first use only",
                detail: model.status)
        case .running:
            runningCanvas
        case .failed(let message):
            failedCanvas(message)
        case .result(let artifact):
            resultCanvas(artifact)
        }
    }

    private var emptyCanvas: some View {
        VStack(spacing: 14) {
            Image(systemName: Design.modalityGlyph(record.modality))
                .font(.system(size: 26, weight: .light))
                .foregroundStyle(.quaternary)
            Text("Describe an image and \(record.name) paints it here.")
                .font(Design.plaque(16))
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
                .frame(maxWidth: 360)
        }
        .padding(28)
    }

    private func waitingCanvas(headline: String, detail: String?) -> some View {
        VStack(spacing: 14) {
            ProgressView()
                .controlSize(.small)
            Text(headline)
                .font(Design.plaque(15))
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
            if let detail, detail != headline {
                Text(detail)
                    .font(.system(size: 11))
                    .foregroundStyle(.tertiary)
                    .multilineTextAlignment(.center)
                    .frame(maxWidth: 400)
            }
            Button("Cancel") {
                model.cancel()
            }
            .controlSize(.small)
            .keyboardShortcut(.cancelAction)
            .padding(.top, 6)
        }
        .padding(28)
    }

    private var runningCanvas: some View {
        VStack(spacing: 0) {
            ZStack {
                if let preview = model.preview {
                    Image(nsImage: preview)
                        .resizable()
                        .interpolation(.medium)
                        .scaledToFit()
                        .clipShape(RoundedRectangle(cornerRadius: 8))
                        .accessibilityLabel(previewDescription)
                } else {
                    RoundedRectangle(cornerRadius: 8)
                        .fill(.quaternary.opacity(0.3))
                        .overlay {
                            ProgressView()
                                .controlSize(.small)
                        }
                        .aspectRatio(1, contentMode: .fit)
                        .frame(maxWidth: 420)
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .padding(.horizontal, 28)
            .padding(.top, 20)
            progressBar
        }
    }

    private var progressBar: some View {
        HStack(spacing: 12) {
            ProgressView(value: model.progress.fraction)
                .progressViewStyle(.linear)
                .controlSize(.small)
                .frame(maxWidth: 260)
            if let step = model.progress.step, let total = model.progress.totalSteps {
                Text("step \(step) / \(total)")
                    .font(Design.data(11))
                    .foregroundStyle(.secondary)
                    .monospacedDigit()
            }
            Spacer()
            Button("Cancel") {
                model.cancel()
            }
            .controlSize(.small)
            .keyboardShortcut(.cancelAction)
        }
        .padding(.horizontal, 28)
        .padding(.vertical, 14)
    }

    private var previewDescription: String {
        if let prompt = model.activePrompt {
            return "Image preview: \(prompt)"
        }
        return "Image preview"
    }

    private func failedCanvas(_ message: String) -> some View {
        VStack(spacing: 12) {
            Image(systemName: "exclamationmark.triangle")
                .font(.system(size: 20, weight: .light))
                .foregroundStyle(Design.warn)
            Text("Generation failed")
                .font(Design.plaque(15))
            Text(message)
                .font(.system(size: 12))
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
                .textSelection(.enabled)
                .frame(maxWidth: 440)
            Button("Copy details") {
                model.copyFailureDetails()
            }
            .controlSize(.small)
            .padding(.top, 6)
        }
        .padding(28)
    }

    private func resultCanvas(_ artifact: Artifact) -> some View {
        VStack(spacing: 12) {
            if let image = model.resultImage {
                Image(nsImage: image)
                    .resizable()
                    .interpolation(.high)
                    .scaledToFit()
                    .clipShape(RoundedRectangle(cornerRadius: 8))
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                    .accessibilityLabel(resultDescription(artifact))
            }
            Text(Provenance.line(for: artifact, schema: model.form.schema))
                .font(Design.data(11))
                .foregroundStyle(.secondary)
                .lineLimit(1)
                .truncationMode(.middle)
                .textSelection(.enabled)
            HStack(spacing: 10) {
                Button("Re-run") {
                    model.rerun(artifact)
                }
                .help("Generate again with the identical seed")
                Button("Vary") {
                    model.vary(artifact)
                }
                .help("Generate a sibling with a fresh seed")
                Button("Reveal in Finder") {
                    model.reveal()
                }
            }
            .controlSize(.small)
        }
        .padding(.horizontal, 28)
        .padding(.top, 20)
        .padding(.bottom, 14)
    }

    private func resultDescription(_ artifact: Artifact) -> String {
        if let prompt = Provenance.prompt(of: artifact.params) {
            return "Generated image: \(prompt)"
        }
        return "Generated image"
    }

    private var controls: some View {
        VStack(spacing: 12) {
            if !record.params.isEmpty {
                paramGrid
            }
            composer
        }
        .padding(.horizontal, 20)
        .padding(.top, 12)
        .padding(.bottom, 16)
    }

    private var paramGrid: some View {
        LazyVGrid(
            columns: [GridItem(.adaptive(minimum: 150, maximum: 260), alignment: .top)],
            alignment: .leading, spacing: 12
        ) {
            ForEach(record.params, id: \.key) { spec in
                VStack(alignment: .leading, spacing: 5) {
                    Text(spec.key.uppercased())
                        .font(.system(size: 9, weight: .medium))
                        .tracking(0.8)
                        .foregroundStyle(.tertiary)
                    control(for: spec)
                }
            }
        }
        .disabled(model.isBusy)
    }

    @ViewBuilder
    private func control(for spec: ParamSpec) -> some View {
        switch spec.type {
        case .int where spec.intRange != nil:
            intSlider(spec, range: spec.intRange!)
        case .int:
            seedField(spec)
        case .float:
            floatSlider(spec)
        case .enumeration:
            enumPicker(spec)
        case .bool:
            boolToggle(spec)
        case .string:
            stringField(spec)
        }
    }

    private func intSlider(_ spec: ParamSpec, range: ClosedRange<Int>) -> some View {
        HStack(spacing: 8) {
            Slider(
                value: Binding(
                    get: { Double(model.form.int(spec.key) ?? range.lowerBound) },
                    set: { model.form.set(spec.key, to: .int(Int($0.rounded()))) }),
                in: Double(range.lowerBound)...Double(range.upperBound),
                step: 1
            ) {
                Text(spec.key)
            }
            .labelsHidden()
            .controlSize(.small)
            Text("\(model.form.int(spec.key) ?? range.lowerBound)")
                .font(Design.data(11))
                .monospacedDigit()
                .frame(minWidth: 24, alignment: .trailing)
        }
    }

    private func floatSlider(_ spec: ParamSpec) -> some View {
        let range = spec.doubleRange ?? 0...1
        let step = spec.doubleStep ?? ParamSpec.step(across: range)
        let decimals = ParamSpec.decimals(forStep: step)
        return HStack(spacing: 8) {
            Slider(
                value: Binding(
                    get: { model.form.double(spec.key) ?? range.lowerBound },
                    set: { model.form.set(spec.key, to: .double(($0 / step).rounded() * step)) }),
                in: range,
                step: step
            ) {
                Text(spec.key)
            }
            .labelsHidden()
            .controlSize(.small)
            Text(
                String(
                    format: "%.\(decimals)f", model.form.double(spec.key) ?? range.lowerBound)
            )
            .font(Design.data(11))
            .monospacedDigit()
            .frame(minWidth: 28, alignment: .trailing)
        }
    }

    private func enumPicker(_ spec: ParamSpec) -> some View {
        Picker(
            spec.key,
            selection: Binding(
                get: { model.form.string(spec.key) ?? spec.values?.first ?? "" },
                set: { model.form.set(spec.key, to: .string($0)) })
        ) {
            ForEach(spec.values ?? [], id: \.self) { value in
                Text(value).tag(value)
            }
        }
        .pickerStyle(.segmented)
        .labelsHidden()
        .controlSize(.small)
    }

    private func boolToggle(_ spec: ParamSpec) -> some View {
        Toggle(
            spec.key,
            isOn: Binding(
                get: { model.form.bool(spec.key) ?? false },
                set: { model.form.set(spec.key, to: .bool($0)) })
        )
        .toggleStyle(.switch)
        .labelsHidden()
        .controlSize(.small)
    }

    private func stringField(_ spec: ParamSpec) -> some View {
        TextField(
            spec.key,
            text: Binding(
                get: { model.form.string(spec.key) ?? "" },
                set: { model.form.set(spec.key, to: .string($0)) })
        )
        .textFieldStyle(.roundedBorder)
        .labelsHidden()
        .controlSize(.small)
    }

    private func seedField(_ spec: ParamSpec) -> some View {
        HStack(spacing: 6) {
            TextField(
                spec.key,
                text: Binding(
                    get: { model.form.int(spec.key).map(String.init) ?? "" },
                    set: { raw in
                        if let value = Int(raw.trimmingCharacters(in: .whitespaces)) {
                            model.form.set(spec.key, to: .int(value))
                        } else if raw.isEmpty {
                            model.form.clear(spec.key)
                        }
                    }),
                prompt: Text("random")
            )
            .textFieldStyle(.roundedBorder)
            .font(Design.data(11))
            .labelsHidden()
            .controlSize(.small)
            Button {
                model.form.roll(spec.key)
            } label: {
                Image(systemName: "dice")
                    .font(.system(size: 11))
            }
            .buttonStyle(.plain)
            .foregroundStyle(.secondary)
            .help("Roll a random \(spec.key)")
        }
    }

    private var composer: some View {
        HStack(alignment: .bottom, spacing: 8) {
            TextField(
                "Describe an image…", text: $model.prompt, axis: .vertical
            )
            .textFieldStyle(.plain)
            .font(.system(size: 13))
            .lineLimit(1...6)
            .onSubmit { model.generate() }
            .disabled(model.isBusy)
            .padding(.leading, 6)
            .padding(.vertical, 3)
            if model.isBusy {
                Button {
                    model.cancel()
                } label: {
                    Image(systemName: "stop.fill")
                        .font(.system(size: 11, weight: .semibold))
                        .foregroundStyle(.white)
                        .frame(width: 26, height: 26)
                        .background(Design.warn, in: Circle())
                }
                .buttonStyle(.plain)
                .keyboardShortcut(.cancelAction)
                .accessibilityLabel("Cancel")
                .help("Cancel the running job")
            } else {
                Button {
                    model.generate()
                } label: {
                    Text("Generate")
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundStyle(generatable ? .white : .secondary)
                        .padding(.horizontal, 12)
                        .frame(height: 26)
                        .background(
                            generatable
                                ? AnyShapeStyle(Design.accent) : AnyShapeStyle(.quaternary),
                            in: Capsule())
                }
                .buttonStyle(.plain)
                .disabled(!generatable)
                .keyboardShortcut(.defaultAction)
                .help("Generate an image")
            }
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 7)
        .background(.quaternary.opacity(0.3), in: RoundedRectangle(cornerRadius: 22))
        .overlay(
            RoundedRectangle(cornerRadius: 22)
                .strokeBorder(.quaternary, lineWidth: 1))
    }

    private var generatable: Bool {
        !model.prompt.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }
}
