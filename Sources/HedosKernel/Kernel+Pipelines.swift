import Foundation

extension Kernel: PipelineBackend {}

extension Kernel {
    public func pipelines() async -> [Pipeline] {
        await pipelineStore.list()
    }

    public func pipeline(id: String) async -> Pipeline? {
        await pipelineStore.get(id: id)
    }

    @discardableResult
    public func savePipeline(_ pipeline: Pipeline) async throws -> Pipeline {
        let shelf = try await registry.list()
        try PipelineValidator.validate(pipeline.stages, shelf: shelf)
        return try await pipelineStore.save(pipeline)
    }

    public func deletePipeline(id: String) async {
        await pipelineStore.delete(id: id)
    }

    public func pipelineSignature(_ pipeline: Pipeline) async -> PipelineSignature? {
        let shelf = (try? await registry.list()) ?? []
        return try? PipelineValidator.validate(pipeline.stages, shelf: shelf)
    }

    public func runPipeline(id: String, input: PipelineInput) async throws
        -> AsyncStream<PipelineEvent>
    {
        guard let pipeline = await pipelineStore.get(id: id) else {
            throw KernelError.pipelineNotFound(id)
        }
        return try await runPipeline(pipeline, input: input)
    }

    public func runPipeline(_ pipeline: Pipeline, input: PipelineInput) async throws
        -> AsyncStream<PipelineEvent>
    {
        let runners = try await pipelineRunners(for: pipeline)
        return PipelineExecutor(stages: runners).run(input: input)
    }

    func pipelineRunners(for pipeline: Pipeline) async throws -> [PipelineStageRunner] {
        let shelf = try await registry.list()
        try PipelineValidator.validate(pipeline.stages, shelf: shelf)
        return pipeline.stages.enumerated().map { index, stage in
            runner(index: index, stage: stage)
        }
    }

    func runner(index: Int, stage: PipelineStage, chat: PipelineRunnerFactory.ChatOverride? = nil)
        -> PipelineStageRunner
    {
        switch stage.capability {
        case .transcribe:
            return PipelineRunnerFactory.transcribe(
                index: index, modelID: stage.modelID, params: stage.params,
                sampleRate: WhisperEngine.expectedSampleRate, backend: self)
        case .speak:
            var voice: String?
            if case .string(let configured)? = stage.params["voice"] { voice = configured }
            return PipelineRunnerFactory.speak(
                index: index, modelID: stage.modelID, params: stage.params, voice: voice,
                backend: self)
        case .image:
            return PipelineRunnerFactory.image(
                index: index, modelID: stage.modelID, params: stage.params, backend: self)
        case .embed:
            return PipelineRunnerFactory.embed(
                index: index, modelID: stage.modelID, params: stage.params, backend: self)
        default:
            return PipelineRunnerFactory.textToText(
                index: index, modelID: stage.modelID, capability: stage.capability,
                params: stage.params, backend: self, chat: chat)
        }
    }
}
