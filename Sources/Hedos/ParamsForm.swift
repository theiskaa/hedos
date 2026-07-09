import HedosKernel
import SwiftUI

struct ParamsForm: View {
    @Binding var form: ParamForm
    var disabled = false

    var body: some View {
        VStack(alignment: .leading, spacing: Design.Space.l) {
            ForEach(form.schema, id: \.key) { spec in
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
        .frame(width: Design.Popover.paramsWidth)
        .disabled(disabled)
    }

    private func formControl(_ spec: ParamSpec) -> ParamControl {
        var roll: (() -> Void)?
        if spec.type == .int && spec.intRange == nil {
            roll = { form.roll(spec.key) }
        }
        return ParamControl(
            spec: spec,
            get: { form.value(spec.key) },
            set: { value in
                if let value {
                    form.set(spec.key, to: value)
                } else {
                    form.clear(spec.key)
                }
            },
            roll: roll)
    }
}
