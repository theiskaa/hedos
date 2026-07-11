import Foundation

public struct DiffusersPipelineProfile: Sendable, Hashable {
    public var modality: Modality
    public var capabilities: [Capability]
    public var params: [ParamSpec]

    public init(modality: Modality, capabilities: [Capability], params: [ParamSpec]) {
        self.modality = modality
        self.capabilities = capabilities
        self.params = params
    }
}

public struct SchedulerFacts: Sendable, Hashable {
    public var className: String?
    public var timestepSpacing: String?

    public init(className: String? = nil, timestepSpacing: String? = nil) {
        self.className = className
        self.timestepSpacing = timestepSpacing
    }
}

public struct PipelineRefinement: Sendable, Hashable {
    public var schedulerClasses: Set<String>
    public var timestepSpacing: String?
    public var nameSignals: Set<String>
    public var paramOverrides: [ParamSpec]

    public init(
        schedulerClasses: Set<String>,
        timestepSpacing: String? = nil,
        nameSignals: Set<String> = [],
        paramOverrides: [ParamSpec]
    ) {
        self.schedulerClasses = schedulerClasses
        self.timestepSpacing = timestepSpacing
        self.nameSignals = nameSignals
        self.paramOverrides = paramOverrides
    }

    public func matches(_ facts: SchedulerFacts, repoHint: String?) -> Bool {
        guard let className = facts.className, schedulerClasses.contains(className) else {
            return false
        }
        if let timestepSpacing, facts.timestepSpacing != timestepSpacing {
            return false
        }
        guard !nameSignals.isEmpty else { return true }
        guard let repoHint else { return false }
        let tokens = Self.nameTokens(of: repoHint)
        return nameSignals.contains { tokens.contains($0) }
    }

    static func nameTokens(of repoHint: String) -> Set<String> {
        Set(
            repoHint.lowercased()
                .split(whereSeparator: { !$0.isLetter && !$0.isNumber })
                .map(String.init))
    }
}

public struct PipelineFamily: Sendable, Hashable {
    public var id: String
    public var classNames: Set<String>
    public var modality: Modality
    public var capabilities: [Capability]
    public var params: [ParamSpec]
    public var refinements: [PipelineRefinement]

    public init(
        id: String,
        classNames: Set<String>,
        modality: Modality,
        capabilities: [Capability] = [],
        params: [ParamSpec] = [],
        refinements: [PipelineRefinement] = []
    ) {
        self.id = id
        self.classNames = classNames
        self.modality = modality
        self.capabilities = capabilities
        self.params = params
        self.refinements = refinements
    }
}

public struct PipelineFamilyRegistry: Sendable {
    public var families: [PipelineFamily]

    public init(families: [PipelineFamily]) {
        self.families = families
    }

    public func family(for className: String) -> PipelineFamily? {
        families.first { $0.classNames.contains(className) }
    }

    public func profile(
        for className: String, scheduler: SchedulerFacts?, repoHint: String? = nil
    ) -> DiffusersPipelineProfile? {
        guard let family = family(for: className) else { return nil }
        var params = family.params
        if let scheduler,
            let refinement = family.refinements.first(where: {
                $0.matches(scheduler, repoHint: repoHint)
            })
        {
            for override in refinement.paramOverrides {
                if let index = params.firstIndex(where: { $0.key == override.key }) {
                    params[index] = override
                } else {
                    params.append(override)
                }
            }
        }
        return DiffusersPipelineProfile(
            modality: family.modality,
            capabilities: family.capabilities,
            params: params)
    }

    static let fluxParams: [ParamSpec] = [
        ParamSpec(key: "steps", type: .int, defaultValue: .int(4), range: [.int(1), .int(50)]),
        ParamSpec(
            key: "guidance", type: .float, defaultValue: .double(4.0),
            range: [.double(0), .double(10)]),
        ParamSpec(
            key: "size", type: .enumeration, defaultValue: .string("1024x1024"),
            values: ["512x512", "768x768", "1024x1024"]),
        ParamSpec(key: "seed", type: .int),
    ]

    static let sd1Params: [ParamSpec] = [
        ParamSpec(key: "steps", type: .int, defaultValue: .int(30), range: [.int(1), .int(75)]),
        ParamSpec(
            key: "guidance", type: .float, defaultValue: .double(7.5),
            range: [.double(0), .double(15)]),
        ParamSpec(
            key: "size", type: .enumeration, defaultValue: .string("512x512"),
            values: ["512x512", "576x576", "640x640", "768x768"]),
        ParamSpec(key: "seed", type: .int),
        ParamSpec(key: "negative_prompt", type: .string),
    ]

    static let sdxlParams: [ParamSpec] = [
        ParamSpec(key: "steps", type: .int, defaultValue: .int(30), range: [.int(1), .int(75)]),
        ParamSpec(
            key: "guidance", type: .float, defaultValue: .double(7.0),
            range: [.double(0), .double(15)]),
        ParamSpec(
            key: "size", type: .enumeration, defaultValue: .string("1024x1024"),
            values: ["768x768", "1024x1024", "1152x896", "896x1152"]),
        ParamSpec(key: "seed", type: .int),
        ParamSpec(key: "negative_prompt", type: .string),
    ]

    static let sd3Params: [ParamSpec] = [
        ParamSpec(key: "steps", type: .int, defaultValue: .int(28), range: [.int(1), .int(75)]),
        ParamSpec(
            key: "guidance", type: .float, defaultValue: .double(7.0),
            range: [.double(0), .double(15)]),
        ParamSpec(
            key: "size", type: .enumeration, defaultValue: .string("1024x1024"),
            values: ["768x768", "1024x1024", "1152x896", "896x1152"]),
        ParamSpec(key: "seed", type: .int),
        ParamSpec(key: "negative_prompt", type: .string),
    ]

    static let pixartParams: [ParamSpec] = [
        ParamSpec(key: "steps", type: .int, defaultValue: .int(20), range: [.int(1), .int(75)]),
        ParamSpec(
            key: "guidance", type: .float, defaultValue: .double(4.5),
            range: [.double(0), .double(15)]),
        ParamSpec(
            key: "size", type: .enumeration, defaultValue: .string("1024x1024"),
            values: ["512x512", "768x768", "1024x1024"]),
        ParamSpec(key: "seed", type: .int),
        ParamSpec(key: "negative_prompt", type: .string),
    ]

    static let kandinskyParams: [ParamSpec] = [
        ParamSpec(key: "steps", type: .int, defaultValue: .int(30), range: [.int(1), .int(75)]),
        ParamSpec(
            key: "guidance", type: .float, defaultValue: .double(4.0),
            range: [.double(0), .double(15)]),
        ParamSpec(
            key: "size", type: .enumeration, defaultValue: .string("768x768"),
            values: ["512x512", "768x768", "1024x1024"]),
        ParamSpec(key: "seed", type: .int),
        ParamSpec(key: "negative_prompt", type: .string),
    ]

    static let lcmParams: [ParamSpec] = [
        ParamSpec(key: "steps", type: .int, defaultValue: .int(4), range: [.int(1), .int(8)]),
        ParamSpec(
            key: "guidance", type: .float, defaultValue: .double(1.5),
            range: [.double(0), .double(2)]),
        ParamSpec(
            key: "size", type: .enumeration, defaultValue: .string("512x512"),
            values: ["512x512", "768x768"]),
        ParamSpec(key: "seed", type: .int),
    ]

    static let turboOverrides: [ParamSpec] = [
        ParamSpec(key: "steps", type: .int, defaultValue: .int(2), range: [.int(1), .int(8)]),
        ParamSpec(
            key: "guidance", type: .float, defaultValue: .double(0.0),
            range: [.double(0), .double(2)]),
    ]

    static let turboRefinement = PipelineRefinement(
        schedulerClasses: ["EulerAncestralDiscreteScheduler"],
        timestepSpacing: "trailing",
        nameSignals: ["turbo", "lightning", "lcm"],
        paramOverrides: turboOverrides)

    public static let builtin = PipelineFamilyRegistry(families: [
        PipelineFamily(
            id: "flux",
            classNames: ["FluxPipeline"],
            modality: .image,
            capabilities: [.image],
            params: fluxParams),
        PipelineFamily(
            id: "stable-diffusion",
            classNames: ["StableDiffusionPipeline"],
            modality: .image,
            capabilities: [.image],
            params: sd1Params,
            refinements: [turboRefinement]),
        PipelineFamily(
            id: "stable-diffusion-xl",
            classNames: ["StableDiffusionXLPipeline"],
            modality: .image,
            capabilities: [.image],
            params: sdxlParams,
            refinements: [turboRefinement]),
        PipelineFamily(
            id: "stable-diffusion-3",
            classNames: ["StableDiffusion3Pipeline"],
            modality: .image,
            capabilities: [.image],
            params: sd3Params),
        PipelineFamily(
            id: "pixart",
            classNames: ["PixArtAlphaPipeline", "PixArtSigmaPipeline"],
            modality: .image,
            capabilities: [.image],
            params: pixartParams),
        PipelineFamily(
            id: "kandinsky",
            classNames: ["KandinskyV22Pipeline", "KandinskyV22CombinedPipeline"],
            modality: .image,
            capabilities: [.image],
            params: kandinskyParams),
        PipelineFamily(
            id: "latent-consistency",
            classNames: ["LatentConsistencyModelPipeline"],
            modality: .image,
            capabilities: [.image],
            params: lcmParams),
        PipelineFamily(
            id: "image-edit",
            classNames: [
                "StableDiffusionImg2ImgPipeline",
                "StableDiffusionInpaintPipeline",
                "StableDiffusionXLImg2ImgPipeline",
                "StableDiffusionXLInpaintPipeline",
                "StableDiffusion3Img2ImgPipeline",
                "StableDiffusion3InpaintPipeline",
                "FluxImg2ImgPipeline",
                "FluxInpaintPipeline",
                "KandinskyV22Img2ImgPipeline",
                "LatentConsistencyModelImg2ImgPipeline",
            ],
            modality: .image),
        PipelineFamily(
            id: "image-upscale",
            classNames: [
                "StableDiffusionUpscalePipeline",
                "StableDiffusionLatentUpscalePipeline",
            ],
            modality: .image),
        PipelineFamily(
            id: "video",
            classNames: [
                "TextToVideoSDPipeline",
                "AnimateDiffPipeline",
                "CogVideoXPipeline",
                "StableVideoDiffusionPipeline",
                "HunyuanVideoPipeline",
                "LTXPipeline",
                "WanPipeline",
            ],
            modality: .video),
        PipelineFamily(
            id: "audio",
            classNames: [
                "AudioLDMPipeline",
                "AudioLDM2Pipeline",
                "MusicLDMPipeline",
                "StableAudioPipeline",
            ],
            modality: .audio),
    ])
}
