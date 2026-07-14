import ArgumentParser

struct GlobalOptions: ParsableArguments {
    @Flag(name: .long, help: "Emit machine-readable JSON instead of formatted text.")
    var json = false
}
