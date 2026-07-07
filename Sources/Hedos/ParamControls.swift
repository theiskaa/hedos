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

    private var current: Int {
        intValue(get()) ?? intValue(spec.defaultValue) ?? range.lowerBound
    }

    var body: some View {
        let span = range.upperBound - range.lowerBound
        HStack(spacing: Design.Space.m) {
            if span <= 128 {
                Slider(
                    value: Binding(
                        get: { Double(current) },
                        set: { set(.int(Int($0.rounded()))) }),
                    in: Double(range.lowerBound)...Double(range.upperBound),
                    step: 1
                ) {
                    Text(spec.key)
                }
                .labelsHidden()
                .controlSize(.small)
            } else {
                Slider(
                    value: Binding(
                        get: { Double(current) },
                        set: { set(.int(Int($0.rounded()))) }),
                    in: Double(range.lowerBound)...Double(range.upperBound)
                ) {
                    Text(spec.key)
                }
                .labelsHidden()
                .controlSize(.small)
            }
            Text("\(current)")
                .font(Design.data(11))
                .monospacedDigit()
                .frame(minWidth: 24, alignment: .trailing)
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
        let current =
            doubleValue(get()) ?? doubleValue(spec.defaultValue) ?? range.lowerBound
        HStack(spacing: Design.Space.m) {
            Slider(
                value: Binding(
                    get: { current },
                    set: { set(.double(($0 / step).rounded() * step)) }),
                in: range,
                step: step
            ) {
                Text(spec.key)
            }
            .labelsHidden()
            .controlSize(.small)
            Text(String(format: "%.\(decimals)f", current))
                .font(Design.data(11))
                .monospacedDigit()
                .frame(minWidth: 28, alignment: .trailing)
        }
    }
}

private struct EnumControl: View {
    let spec: ParamSpec
    let get: () -> JSONValue?
    let set: (JSONValue?) -> Void

    var body: some View {
        Picker(
            spec.key,
            selection: Binding(
                get: {
                    stringValue(get()) ?? stringValue(spec.defaultValue)
                        ?? spec.values?.first ?? ""
                },
                set: { set(.string($0)) })
        ) {
            ForEach(spec.values ?? [], id: \.self) { value in
                Text(value).tag(value)
            }
        }
        .pickerStyle(.segmented)
        .labelsHidden()
        .controlSize(.small)
    }
}

private struct BoolControl: View {
    let spec: ParamSpec
    let get: () -> JSONValue?
    let set: (JSONValue?) -> Void

    var body: some View {
        Toggle(
            spec.key,
            isOn: Binding(
                get: { boolValue(get()) ?? boolValue(spec.defaultValue) ?? false },
                set: { set(.bool($0)) })
        )
        .toggleStyle(.switch)
        .labelsHidden()
        .controlSize(.small)
    }
}

private struct StringControl: View {
    let spec: ParamSpec
    let get: () -> JSONValue?
    let set: (JSONValue?) -> Void

    var body: some View {
        TextField(
            spec.key,
            text: Binding(
                get: { stringValue(get()) ?? "" },
                set: { set($0.isEmpty ? nil : .string($0)) })
        )
        .textFieldStyle(.plain)
        .font(Design.caption)
        .padding(.horizontal, Design.Space.m)
        .padding(.vertical, Design.Space.xs)
        .surfaceCard(radius: Design.Radius.card)
        .labelsHidden()
    }
}

private struct FreeIntControl: View {
    let spec: ParamSpec
    let get: () -> JSONValue?
    let set: (JSONValue?) -> Void
    let roll: (() -> Void)?

    var body: some View {
        HStack(spacing: Design.Space.s) {
            TextField(
                spec.key,
                text: Binding(
                    get: { intValue(get()).map(String.init) ?? "" },
                    set: { raw in
                        if let value = Int(raw.trimmingCharacters(in: .whitespaces)) {
                            set(.int(value))
                        } else if raw.isEmpty {
                            set(nil)
                        }
                    }),
                prompt: Text(roll == nil ? "unset" : "random")
            )
            .textFieldStyle(.plain)
            .font(Design.data(11))
            .padding(.horizontal, Design.Space.m)
            .padding(.vertical, Design.Space.xs)
            .surfaceCard(radius: Design.Radius.card)
            .labelsHidden()
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
}
