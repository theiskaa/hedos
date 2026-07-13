# Themes

Hedos appearance has two independent axes, both set in Settings → Appearance:

- **Mode** — System, Light, or Dark. System follows the macOS appearance.
- **Theme** — the palette family. Four ship built in: **Default**, **Gruvbox**, **Solarized**, and **Catppuccin**.

A theme is a single TOML file that carries *both* the light and the dark palette. When you pick a theme, the mode axis chooses which half of that file is shown; switching System/Light/Dark never changes the theme, only which palette within it is active.

## Where theme files live

Each built-in theme has a file bundled inside the app at `Resources/Themes/<id>.toml`, and a matching base palette compiled into `Sources/Hedos/Theme.swift`. At load time Hedos first looks for a user file at:

```
~/.config/hedos/themes/<id>.toml
```

If that file exists, it wins over the bundled one. The `<id>` values are `default`, `gruvbox`, `solarized`, and `catppuccin`.

This is the override mechanism: to reskin **Gruvbox**, create `~/.config/hedos/themes/gruvbox.toml`. Your file is applied *on top of* the built-in Gruvbox palette, so you only need to list the keys you want to change — everything you omit keeps its built-in value. Relaunch Hedos (or reselect the theme) to see the change; no rebuild is required.

## Anatomy of a theme file

A file has a `[meta]` block, a `[light.*]` and `[dark.*]` group of palette blocks, and an optional `[shape]` block. Colors are **quoted hex strings** — `"#RRGGBB"`. (The quotes matter: an unquoted `#RRGGBB` is read as a comment and ignored.)

```toml
[meta]
name = "Gruvbox"

[light.surface]
ground      = "#FBF1C7"   # app background
panel       = "#F2E5BC"   # sidebar and side panels
card        = "#F9F5D7"   # primary card / surface fill
card2       = "#EBDBB2"   # nested / secondary card fill
line        = "#D5C4A1"   # hairline borders and dividers
line_bright = "#BDAE93"   # stronger borders

[light.ink]
text  = "#3C3836"          # primary text
muted = "#665C54"          # secondary text
faint = "#928374"          # tertiary / disabled text

[light.accent]
value = "#3C3836"          # accent (in the monochrome design this tracks ink.text)
dim   = "#665C54"          # dimmed accent
on    = "#FBF1C7"          # text/icons drawn on top of an accent fill

[light.heat]
warm  = "#B57614"          # warm signal (heat, highlights)
error = "#9D0006"          # error / destructive

# ... then the same four blocks under [dark.surface], [dark.ink],
#     [dark.accent], [dark.heat] for the dark palette ...

[shape]
radius_control  = 8        # buttons, fields, small controls
radius_card     = 12       # cards and surfaces
radius_bubble   = 16       # chat bubbles
radius_artifact = 14       # image / artifact cards
unit            = 16       # base spacing unit
hairline        = 1        # hairline stroke width
```

The fourteen color tokens (six `surface`, three `ink`, three `accent`, two `heat`) are the whole palette. `[shape]` is mode-agnostic — it is not split into light/dark — and every field is optional, falling back to the defaults shown above.

Any key you leave out of a mode falls back to a mode-less block of the same name (e.g. a bare `[surface]` shared by both), and then to the built-in value. So a minimal override is short:

```toml
# ~/.config/hedos/themes/default.toml — nudge only the dark background
[dark.surface]
ground = "#000000"
```

## Adding a wholly new theme

> **Not yet supported from user space.** Today the override above only reskins the four built-in themes — dropping a brand-new name into `~/.config/hedos/themes/` will *not* add it to the picker. A wholly new theme currently needs a small code change and a rebuild, as below. Letting a new theme be added purely from the user folder, with no rebuild, is planned.

The theme picker lists a fixed set of families, so for now a brand-new name is added in code. In `Sources/Hedos/Theme.swift`:

1. Add a `ThemeFamily` constant with an `id`, a display `name`, and a base `light`/`dark` `ThemePalette` (the compiled fallback; the TOML then refines it):

   ```swift
   static let nord = ThemeFamily(
       id: "nord",
       name: "Nord",
       light: ThemePalette(/* … fourteen values … */),
       dark: ThemePalette(/* … fourteen values … */))
   ```

2. Add it to the picker list:

   ```swift
   static let all: [ThemeFamily] = [.standard, .gruvbox, .solarized, .catppuccin, .nord]
   ```

3. Optionally drop `Sources/Hedos/Resources/Themes/nord.toml` (using the schema above) so the palette lives in TOML like the others. Users can then still override it via `~/.config/hedos/themes/nord.toml`.

After `make app`, the new theme appears as a card in Settings → Appearance.
