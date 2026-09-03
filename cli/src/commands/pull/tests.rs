use super::*;

use clap::Parser;

/// The command line `pull` actually hangs off, so the tests read the arguments
/// through the same shape `main` does.
#[derive(Parser)]
#[command(name = "hedos")]
struct Probe {
    #[command(subcommand)]
    command: ProbeCommand,
}

#[derive(Subcommand)]
enum ProbeCommand {
    Pull(PullArgs),
}

fn parse(argv: &[&str]) -> PullArgs {
    match Probe::try_parse_from(argv).map(|probe| probe.command) {
        Ok(ProbeCommand::Pull(args)) => args,
        Err(error) => panic!("{argv:?} should parse: {error}"),
    }
}

#[test]
fn a_reference_is_a_reference_and_not_a_subcommand() {
    let args = parse(&["hedos", "pull", "Qwen/Qwen3-8B"]);
    assert_eq!(args.reference.as_deref(), Some("Qwen/Qwen3-8B"));
    assert!(args.command.is_none());
}

#[test]
fn a_subcommand_wins_over_a_bare_word() {
    let args = parse(&["hedos", "pull", "ls"]);
    assert!(matches!(args.command, Some(PullCommand::Ls)));
    assert_eq!(args.reference, None);
}

#[test]
fn the_value_terminator_names_a_model_a_subcommand_shadows() {
    // A bare word is a valid Ollama tag, so a model actually called `ls` needs a
    // way to be said. This is it.
    let args = parse(&["hedos", "pull", "--", "ls"]);
    assert_eq!(args.reference.as_deref(), Some("ls"));
    assert!(args.command.is_none());
}

#[test]
fn nothing_at_all_is_the_interactive_picker() {
    let args = parse(&["hedos", "pull"]);
    assert_eq!(args.reference, None);
    assert!(args.command.is_none());
    assert!(!args.detach);
}

#[test]
fn detaching_and_forcing_a_provider_ride_along_with_the_reference() {
    let args = parse(&["hedos", "pull", "gemma3:4b", "--from", "ollama", "-d"]);
    assert_eq!(args.reference.as_deref(), Some("gemma3:4b"));
    assert_eq!(args.from.as_deref(), Some("ollama"));
    assert!(args.detach);
}

#[test]
fn a_pull_is_named_by_a_positional_on_every_subcommand_that_acts_on_one() {
    let args = parse(&["hedos", "pull", "cancel", "1700-qwen"]);
    let Some(PullCommand::Cancel(cancel)) = &args.command else {
        panic!("cancel should parse as a subcommand");
    };
    assert_eq!(cancel.job, "1700-qwen");
}

#[test]
fn resume_takes_one_pull_or_all_of_them_but_never_both() {
    let args = parse(&["hedos", "pull", "resume", "--all"]);
    let Some(PullCommand::Resume(resume)) = &args.command else {
        panic!("resume should parse as a subcommand");
    };
    assert!(resume.all);
    assert_eq!(resume.job, None);

    assert!(Probe::try_parse_from(["hedos", "pull", "resume", "abc", "--all"]).is_err());
}

#[test]
fn logs_and_clean_take_their_counts() {
    let args = parse(&["hedos", "pull", "logs", "abc", "-n", "20"]);
    let Some(PullCommand::Logs(logs)) = &args.command else {
        panic!("logs should parse as a subcommand");
    };
    assert_eq!(logs.lines, Some(20));

    let args = parse(&["hedos", "pull", "clean", "--keep", "5"]);
    let Some(PullCommand::Clean(clean)) = &args.command else {
        panic!("clean should parse as a subcommand");
    };
    assert_eq!(clean.keep, 5);
}

#[test]
fn the_provider_is_inferred_from_the_shape_of_the_reference() {
    assert_eq!(
        provider_for("Qwen/Qwen3-8B", None).expect("a repo is a hub repo"),
        InstallProviderId::huggingface()
    );
    assert_eq!(
        provider_for("gemma3:4b", None).expect("a tag is an ollama tag"),
        InstallProviderId::ollama()
    );
    assert_eq!(
        provider_for("Qwen/Qwen3-8B", Some("ollama")).expect("an explicit provider wins"),
        InstallProviderId::ollama()
    );
    assert!(provider_for("gemma3:4b", Some("nowhere")).is_err());
}
