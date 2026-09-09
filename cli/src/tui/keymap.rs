//! The shelf's keys, each spelled once: the footer, the help, the task
//! strip and the empty shelf all read their `key verb` pairs from here, and
//! the reducer's tests check that every binding does something. Dispatch
//! happens in `App::key`; `App::actions` lists the keys the selected model
//! answers to, in footer order. A modal's own keys (`enter choose`, `esc
//! back`, `y remove`, `n keep`, `esc close`, the chat pane's footer) are
//! named where they are drawn: they answer only inside their card and never
//! share the shelf's grammar.
//!
//! A capital letter is normally the sibling of its lowercase (`t`/`T`, `y`/`Y`,
//! `g`/`G`, `p`/`P`). `R` is the exception: `r` is taken by an unrelated shelf
//! verb and no free lowercase says "resume".
//!
//! A screen that takes the shelf's place answers to a few of the shelf's keys
//! with the same verbs and to a couple of its own; [`ScreenKeys`] holds both
//! halves, and [`PULLS`] and [`BENCH`] are the two of them.

/// A key and its verb, as every key line draws them.
pub type Pair = (&'static str, &'static str);

/// What a binding is about; the help lays each group out under its own
/// heading.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    /// Moving the selection and opening it up.
    Move,
    /// Verbs on the selected model.
    Model,
    /// Verbs on the shelf as a whole.
    Shelf,
    /// Verbs on the screen itself.
    Screen,
    /// The pulls screen's own keys; the help files them with the shelf keys
    /// the screen shares.
    Pulls,
    /// The bench screen's own keys, filed the same way.
    Bench,
}

impl Group {
    /// The heading the help puts over the group.
    pub fn label(self) -> &'static str {
        match self {
            Group::Move => "MOVE",
            Group::Model => "MODEL",
            Group::Shelf => "SHELF",
            Group::Screen => "SCREEN",
            Group::Pulls => "PULLS",
            Group::Bench => "BENCH",
        }
    }
}

/// One key and what it does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Binding {
    /// The key as the hints name it: `w`, `j/k`, `enter`.
    pub key: &'static str,
    /// The verb the footer and the strip show beside the key.
    pub verb: &'static str,
    /// A fuller phrasing for the help, where there is room for one.
    pub gloss: Option<&'static str>,
    /// Where the help files it.
    pub group: Group,
}

impl Binding {
    /// The gloss, or the verb for a binding without one.
    pub fn gloss(&self) -> &'static str {
        self.gloss.unwrap_or(self.verb)
    }
}

const fn bind(key: &'static str, verb: &'static str, group: Group) -> Binding {
    Binding {
        key,
        verb,
        gloss: None,
        group,
    }
}

const fn glossed(
    key: &'static str,
    verb: &'static str,
    gloss: &'static str,
    group: Group,
) -> Binding {
    Binding {
        key,
        verb,
        gloss: Some(gloss),
        group,
    }
}

/// Every key the shelf answers to.
pub const BINDINGS: &[Binding] = &[
    bind("j/k", "move", Group::Move),
    bind("↑/↓", "move", Group::Move),
    bind("g/G", "top / bottom", Group::Move),
    bind("enter", "expand", Group::Move),
    glossed("esc", "collapse", "collapse / clear", Group::Move),
    bind("w", "warm", Group::Model),
    bind("u", "unload", Group::Model),
    glossed("l", "launch", "launch a harness", Group::Model),
    glossed("t", "try", "try here", Group::Model),
    glossed("b", "bench", "measure it here", Group::Model),
    glossed("T", "chat", "chat in terminal", Group::Model),
    bind("x", "remove", Group::Model),
    bind("y", "copy path", Group::Model),
    glossed("Y", "copy id", "id", Group::Model),
    bind("p", "pull", Group::Shelf),
    bind("P", "pulls", Group::Shelf),
    glossed("B", "benches", "the bench screen", Group::Shelf),
    bind("s", "scan", Group::Shelf),
    bind("/", "filter", Group::Shelf),
    bind("o", "sort", Group::Shelf),
    bind("r", "refresh", Group::Shelf),
    glossed("c", "stop", "stop pull", Group::Shelf),
    glossed("R", "resume", "resume pull", Group::Shelf),
    bind("d", "dismiss", Group::Shelf),
    bind("S", "serve", Group::Screen),
    bind("?", "help", Group::Screen),
    bind("q", "quit", Group::Screen),
];

/// A screen that takes the shelf's place: the shelf keys it answers to with
/// the shelf's own verbs, and the keys that are its alone.
pub struct ScreenKeys {
    /// Shelf keys the screen answers to, resolved through [`BINDINGS`].
    shared: &'static [&'static str],
    /// Keys the screen alone answers to.
    own: &'static [Binding],
}

/// The pulls screen, on top of the screen group's `?` and `q`. `P` closes it
/// too, as the sibling of the `P` that opened it. Its own `x` forgets an ended
/// pull's record, which is what `hedos pull clean` does to all of them.
pub const PULLS: ScreenKeys = ScreenKeys {
    shared: &["j/k", "↑/↓", "g/G", "p", "c", "R", "Y"],
    own: &[
        bind("x", "forget", Group::Pulls),
        bind("esc", "shelf", Group::Pulls),
    ],
};

/// The bench screen, the same way. `B` closes it as the sibling of the `B`
/// that opened it, and `c` stops the bench the way it stops a pull.
pub const BENCH: ScreenKeys = ScreenKeys {
    shared: &["j/k", "↑/↓", "g/G", "b", "c"],
    own: &[
        glossed("a", "all", "bench every model", Group::Bench),
        bind("esc", "shelf", Group::Bench),
    ],
};

impl ScreenKeys {
    /// Every key the screen answers to: the shared ones with their shelf
    /// groups, then its own.
    pub fn bindings(&self) -> impl Iterator<Item = &'static Binding> + use<'_> {
        self.shared
            .iter()
            .filter_map(|key| binding(key))
            .chain(self.own.iter())
    }

    /// The screen's keys as the help lists them: everything but moving, which
    /// reads as it does on the shelf.
    pub fn help_bindings(&self) -> impl Iterator<Item = &'static Binding> + use<'_> {
        self.bindings()
            .filter(|binding| binding.group != Group::Move)
    }

    /// `(key, verb)` pairs for `keys`, in that order, skipping any the screen
    /// does not answer to.
    pub fn pairs(&self, keys: &[&str]) -> Vec<Pair> {
        keys.iter()
            .filter_map(|key| self.bindings().find(|binding| binding.key == *key))
            .map(|binding| (binding.key, binding.verb))
            .collect()
    }

    /// The verb the screen gives `key`, or its gloss where it has one.
    pub fn gloss(&self, key: &str) -> &'static str {
        self.bindings()
            .find(|binding| binding.key == key)
            .map_or("", Binding::gloss)
    }
}

/// The binding for `key`, if there is one.
pub fn binding(key: &str) -> Option<&'static Binding> {
    BINDINGS.iter().find(|binding| binding.key == key)
}

/// The verb bound to `key`; empty for a key that is not bound, which the
/// tests of every consumer rule out.
pub fn verb(key: &str) -> &'static str {
    binding(key).map_or("", |binding| binding.verb)
}

/// `(key, verb)` pairs for `keys`, in that order, skipping any that is not
/// bound.
pub fn pairs(keys: &[&str]) -> Vec<Pair> {
    keys.iter()
        .filter_map(|key| binding(key))
        .map(|binding| (binding.key, binding.verb))
        .collect()
}

/// The single ASCII characters a key names, the ones the reducer sees as
/// `Key::Char`: `w` is itself, `j/k` is both of them, `/` is itself, and a
/// named key like `enter` or an arrow pair like `↑/↓` is none.
#[cfg(test)]
pub fn chars(key: &str) -> Vec<char> {
    let single = |part: &str| {
        let mut chars = part.chars();
        match (chars.next(), chars.next()) {
            (Some(only), None) if only.is_ascii() => Some(only),
            _ => None,
        }
    };
    match single(key) {
        Some(only) => vec![only],
        None => key.split('/').filter_map(single).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_bound_once() {
        for (index, binding) in BINDINGS.iter().enumerate() {
            assert!(
                !BINDINGS[..index]
                    .iter()
                    .any(|other| other.key == binding.key),
                "{} is bound twice",
                binding.key
            );
        }
    }

    #[test]
    fn every_screen_key_resolves_to_a_shelf_binding_plus_the_screens_own() {
        for (screen, group) in [(&PULLS, Group::Pulls), (&BENCH, Group::Bench)] {
            assert_eq!(
                screen.bindings().count(),
                screen.shared.len() + screen.own.len()
            );
            for own in screen.own {
                assert_eq!(own.group, group);
                assert!(
                    !screen.shared.contains(&own.key),
                    "{} is both shared and the screen's own",
                    own.key
                );
            }
        }
        assert_eq!(
            PULLS.pairs(&["j/k", "w", "x", "esc"]),
            vec![("j/k", "move"), ("x", "forget"), ("esc", "shelf")]
        );
        assert_eq!(
            BENCH.pairs(&["j/k", "b", "a", "esc"]),
            vec![
                ("j/k", "move"),
                ("b", "bench"),
                ("a", "all"),
                ("esc", "shelf")
            ]
        );
        assert_eq!(BENCH.gloss("a"), "bench every model");
        assert_eq!(BENCH.gloss("nope"), "");
    }

    #[test]
    fn lookups_skip_unbound_keys() {
        assert_eq!(verb("w"), "warm");
        assert_eq!(verb("nope"), "");
        assert_eq!(
            pairs(&["j/k", "nope", "q"]),
            vec![("j/k", "move"), ("q", "quit")]
        );
        assert_eq!(binding("Y").map(Binding::gloss), Some("id"));
        assert_eq!(binding("w").map(Binding::gloss), Some("warm"));
    }

    #[test]
    fn chars_splits_only_paired_keys() {
        assert_eq!(chars("w"), vec!['w']);
        assert_eq!(chars("/"), vec!['/']);
        assert_eq!(chars("j/k"), vec!['j', 'k']);
        assert!(chars("enter").is_empty());
        assert!(chars("↑/↓").is_empty());
    }
}
