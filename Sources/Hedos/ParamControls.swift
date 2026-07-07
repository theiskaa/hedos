import HedosKernel
import SwiftUI

struct ParamControl: View {
    let spec: ParamSpec
    let get: () -> JSONValue?
    let set: (JSONValue?) -> Void
    var roll: (() -> Void)? = nil

    var body: some View {
        switch spec.type {
        case .int where spec.intRange != nil:
            IntSliderControl(spec: spec, range: spec.intRange!, get: get, set: set)
        case .int:
            FreeIntControl(spec: spec, get: get, set: set, roll: roll)
        case .float:
            FloatSliderControl(spec: spec, get: get, set: set)
        case .enumeration:
            EnumControl(spec: spec, get: get, set: set)
        case .bool:
            BoolControl(spec: spec, get: get, set: set)
        case .string:
            StringControl(spec: spec, get: get, set: set)
        }
    }
}

private func intValue(_ value: JSONValue?) -> Int? {
    switch value {
    case .int(let raw): raw
    case .double(let raw): Int(raw)
    default: nil
    }
}

private func doubleValue(_ value: JSONValue?) -> Double? {
    switch value {
    case .double(let raw): raw
    case .int(let raw): Double(raw)
    default: nil
    }
}

private func stringValue(_ value: JSONValue?) -> String? {
    if case .string(let raw) = value { return raw }
    return nil
}

private func boolValue(_ value: JSONValue?) -> Bool? {
    if case .bool(let raw) = value { return raw }
    return nil
}

private struct IntSliderControl: View {
    let spec: ParamSpec
    let range: ClosedRange<Int>
    let get: () -> JSONValue?
    let set: (JSONValue?) -> Void

    private var stored: Int? { intValue(get()) }

    private var thumb: Int {
        stored ?? intValue(spec.defaultValue)
            ?? (range.lowerBound + (range.upperBound - range.lowerBound) / 2)
    }

    var body: some View {
        HStack(spacing: Design.Space.m) {
            InkSlider(
                range: Double(range.lowerBound)...Double(range.upperBound),
                value: Double(thumb),
                isSet: stored != nil,
                onChange: { set(.int(Int($0.rounded()))) },
                label: spec.key)
            Text(stored.map(String.init) ?? "auto")
                .font(Design.data(11))
                .monospacedDigit()
                .foregroundStyle(stored == nil ? Design.inkFaint : Design.ink)
                .frame(minWidth: 34, alignment: .trailing)
        }
    }
}

private struct FloatSliderControl: View {
    let spec: ParamSpec
    let get: () -> JSONValue?
    let set: (JSONValue?) -> Void

    var body: some View {
        let range = spec.doubleRange ?? 0...1
        let step = spec.doubleStep ?? ParamSpec.step(across: range)
        let decimals = ParamSpec.decimals(forStep: step)
        let stored = doubleValue(get())
        let thumb =
            stored ?? doubleValue(spec.defaultValue)
            ?? (range.lowerBound + range.upperBound) / 2
        HStack(spacing: Design.Space.m) {
            InkSlider(
                range: range,
                value: thumb,
                isSet: stored != nil,
                onChange: { set(.double(($0 / step).rounded() * step)) },
                label: spec.key)
            Text(stored.map { String(format: "%.\(decimals)f", $0) } ?? "auto")
                .font(Design.data(11))
                .monospacedDigit()
                .foregroundStyle(stored == nil ? Design.inkFaint : Design.ink)
                .frame(minWidth: 34, alignment: .trailing)
        }
    }
}

private struct EnumControl: View {
    let spec: ParamSpec
    let get: () -> JSONValue?
    let set: (JSONValue?) -> Void

    var body: some View {
        let values = spec.values ?? []
        if values.count > 4 || values.contains(where: { $0.count > 6 }) {
            InkDropdown(
                options: values,
                selection: stringValue(get()) ?? stringValue(spec.defaultValue),
                allowsAuto: false,
                accessibilityName: spec.key,
                onSelect: { choice in
                    if let choice {
                        set(.string(choice))
                    }
                })
        } else {
            InkSegmented(
                values: values,
                selection: stringValue(get()) ?? stringValue(spec.defaultValue),
                onSelect: { set(.string($0)) })
        }
    }
}

private struct BoolControl: View {
    let spec: ParamSpec
    let get: () -> JSONValue?
    let set: (JSONValue?) -> Void

    var body: some View {
        HStack(spacing: Design.Space.m) {
            if boolValue(get()) == nil {
                Text("auto")
                    .font(Design.data(11))
                    .foregroundStyle(Design.inkFaint)
            }
            InkToggle(
                isOn: boolValue(get()) ?? boolValue(spec.defaultValue) ?? false,
                isSet: boolValue(get()) != nil,
                onToggle: { set(.bool($0)) },
                label: spec.key)
        }
    }
}

private struct StringControl: View {
    let spec: ParamSpec
    let get: () -> JSONValue?
    let set: (JSONValue?) -> Void
    @State private var draft = ""
    @State private var seeded = false

    var body: some View {
        InkField(
            placeholder: spec.key,
            text: $draft,
            onSubmit: { commit() },
            onFocusLost: { commit() }
        )
        .onAppear {
            guard !seeded else { return }
            draft = stringValue(get()) ?? ""
            seeded = true
        }
        .accessibilityLabel(spec.key)
    }

    private func commit() {
        set(draft.isEmpty ? nil : .string(draft))
    }
}

private struct FreeIntControl: View {
    let spec: ParamSpec
    let get: () -> JSONValue?
    let set: (JSONValue?) -> Void
    let roll: (() -> Void)?
    @State private var draft = ""
    @State private var seeded = false

    var body: some View {
        HStack(spacing: Design.Space.s) {
            InkField(
                placeholder: roll == nil ? "unset" : "random",
                text: $draft,
                font: Design.data(11),
                onSubmit: { commit() },
                onFocusLost: { commit() }
            )
            .onAppear {
                guard !seeded else { return }
                draft = intValue(get()).map(String.init) ?? ""
                seeded = true
            }
            .onChange(of: intValue(get())) { _, fresh in
                let current = Int(draft.trimmingCharacters(in: .whitespaces))
                if fresh != current {
                    draft = fresh.map(String.init) ?? ""
                }
            }
            .accessibilityLabel(spec.key)
            if let roll {
                Button(action: roll) {
                    Image(systemName: "dice")
                        .font(Design.label)
                }
                .buttonStyle(.plain)
                .foregroundStyle(Design.inkSoft)
                .help("Roll a random \(spec.key)")
                .accessibilityLabel("Roll a random \(spec.key)")
            }
        }
    }

    private func commit() {
        let trimmed = draft.trimmingCharacters(in: .whitespaces)
        if let value = Int(trimmed) {
            set(.int(value))
        } else if trimmed.isEmpty {
            set(nil)
        }
    }
}
