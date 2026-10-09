# The shelf

`hedos shelf` is the shelf as a screen you keep open. It shows every model with its runtime, store, and size; the selected one's fit, residency, and gateway traffic; what is loaded and by whom; disk per store; and what the gateway has served. From the same screen you can warm and unload models, remove them, pull new ones, talk to one, bench them, and hand the terminal to a coding harness.

Every key is a subcommand: `p` is `hedos pull`, `x` is `hedos rm`, `w` is `hedos warm`. The footer shows only the keys that apply to the model under the cursor, and `?` lists them all.

```sh
hedos shelf
```

It takes no flags and needs a terminal on both stdin and stdout. It works over ssh and inside tmux; see [Over ssh and in tmux](#over-ssh-and-in-tmux).

## Opening the shelf

When the screen opens, hedos:

1. scans the machine if the shelf is empty,
2. drops the records of ended pulls past `pull.keep_ended`,
3. starts again every pull whose worker died, when `pull.auto_resume` is on (the default); a pull you paused stays paused,
4. reads the pull directory, so a download already under way is in the task strip on the first frame,
5. puts the selection back on the model it was on last time.

If the shelf is still empty after the scan, the screen opens on the [pull screen](#the-pull-screen), since that is the useful first thing to do. Close it with `esc` to see the empty shelf, which says where hedos looks (the Ollama store, the Hugging Face cache, LM Studio, and loose GGUF or safetensors files in your folders) and offers `p` to pull a model and `s` to scan again.

Between runs, the shelf remembers the selected model and which failed pulls you dismissed from the strip. It keeps them in `ui/ui.toml` under the [data directory](configuration.md#the-data-directory). The filter is not remembered.

## Layout

From top to bottom the screen has a header, a body of cards, a task strip when there is background work, and a one-line footer of keys. The UI paints no ground of its own: it draws on your terminal's background (see [colour](#colour)).

The try screen and the pull screen take the whole body in the shelf's place and hide the task strip; the pull screen keeps its own downloads card instead. The pulls and bench screens keep the header, the strip and the footer, with their list where the shelf goes and the selected row's detail where the model card goes.

### The header

On a terminal of at least 96 columns and 40 rows, the header is the hero:

- the koala on the left,
- the `hedos` wordmark in pixel type with the version, the tagline `one home for every local model on your machine`, and `ἕδος · the place where something comes to rest` under it,
- and on the right, the gateway's pulse:
  - `● GATEWAY ON  127.0.0.1:43367` with its requests a minute at the right, or `○ GATEWAY OFF` and `S serve`,
  - under it, the requests it served over the last day as two rows of bars, oldest at the left, each column a half hour on a wide terminal and up to about an hour on the narrowest. The busiest column fills both rows, a quiet stretch is a low rule, and while the gateway is on, the newest column is the brightest once it has served something. While it is off the day stays, greyed, with its quiet half hours left empty,
  - then `24h ago` and `now` under the bars,
- and on the tagline's rows, against the right edge, the shelf and the memory:
  - `19 models · 3 warm · 3 gone`, the gone count only when something is gone,
  - `18.6 GiB held · 45 GiB free of 64`: what the loaded models hold, or `nothing held`, then the free memory of the machine's total when hedos knows it.

On a short row the badge drops the address to the port, then the word `GATEWAY`, and only then the rate. The tagline and the gloss are each cut short before they run into the count line beside them. The counts count up from zero when the screen opens and ease to each new value.

On a smaller terminal, the header is one line: the wordmark and version, then `12 models · 3 warm · 1 too big · 2 gone` (the last two only when they count something), and against the right edge the gateway (`● :43367`, or `○ gateway off`). When no machine card is on screen, it adds the free memory. On a narrow terminal the too-big and gone counts go first, so the right side always survives.

### The shelf card

The shelf card is the same table `hedos ls` prints, titled `shelf · <count>`, with the sort on its right edge (`by name`).

| Column | Shows |
| --- | --- |
| (gutter) | `●` warm, `○` cold, `✕` weights gone, or a spinner while a warm, unload, or removal runs on the model. |
| NAME | The model's name, bold when it is warm or selected. |
| RUNTIME | The runtime it resolved to, shortened. |
| STORE | Where it lives (`hf` for the Hugging Face cache, `lm studio` for LM Studio). |
| SIZE | What serving it loads, with `tight` or `too big` after it when it does not simply fit, and `gone` when its weights are gone. |

A row that is too big for the machine, or whose weights are gone, is drawn dim. On a narrow card the STORE column goes first, then RUNTIME. A long shelf scrolls with the selection.

### The model card

Beside the shelf, the model card describes the selected model. Its right edge says where the selection is (`3 of 12`).

- **The name** arrives letter by letter, with a line of facts under it: runtime, store, serving size, context length, and the disk size when it differs from the serving size (`8.5 GB · ctx 32k · 34 GB on disk`).
- **The capabilities** as chips (`chat`, `tools`, `see`, ...).
- **MEMORY**:
  - `fit`: the verdict and what it needs (`fits · needs 4.7 of 64 GiB`), then how much stays free beside what is already loaded, or that it won't fit beside it.
  - a gauge of the machine's memory: what the other loaded models hold, what this one needs (`held` when it is loaded, `if warmed` when it is not), and the rest.
  - `residency`: `● warm` and who holds it (`this process`, `Ollama daemon`, or `gateway :<port>`), with `unloads in 4m` when it has a warm window, or `cold`.
- **GATEWAY**: when the model was last used through the gateway, the requests served in the last 24 hours with their p50, p90 and p99 latency, and a bar per hour for the last day. A model that never came through the gateway says `no requests through the gateway`.
- **path**: where the weights are, at the foot of the card, with `· gone` after it when the file is no longer there.

Press `enter` to expand the card over the whole body. Expanded, it also shows a RECORD section: the id, runtime id, store id, alias, modality, execution mode, and state. `esc` or `enter` collapses it.

### The machine card

Under the shelf, the machine card shows the machine's total memory on its right edge and three lines:

- **memory**: a bar with one segment per loaded model, and the GiB loaded of the total.
- **a legend** naming each segment with its GiB, then the free memory, or `nothing loaded`.
- **disk**: the bytes on disk across every store, then each store's share (`40.3 GB · ollama 27.8 GB · hf 12.4 GB`).

The disk line counts the way [`hedos scan`](cli.md#what-the-sizes-count) does, each file once, so the figures match. It counts in a separate process in the background and shows `counting` until the first count finishes, then the last count while another runs. A slow or stalled disk never holds up the screen, and quitting never waits for a count.

### The gateway card

Beside the machine card, the gateway card names the dialects it speaks (`openai · ollama · anthropic`) and shows:

- `● on · 127.0.0.1:<port> · <n> req/min` with the dot pulsing, or `○ off` and `S serve`,
- `last request 21d ago · 11,376 requests all time`, or `nothing served yet`,
- a bar per hour of the last day's requests, when there were any.

A running `hedos serve` on the configured port is detected, and the models it has loaded count as warm on the shelf. The traffic comes from the gateway's audit log, the same one [`hedos stats`](cli.md#hedos-stats) reads.

### The task strip

The `tasks` card appears under the body while there is background work: scans, warms, unloads, removals, pulls, and a row for each hand-off once it ends. It shows up to four rows. A finished row stays for a minute, a failed one for ten. A key hint sits on the one row it acts on:

| Hint | On | Does |
| --- | --- | --- |
| `c` | The newest pull still downloading. | Opens the stop card. |
| `R` | The newest pull that stopped and can go on. | Resumes it. |
| `w` `l` | A pull that landed, while its model is selected. | Warm it, or launch a harness on it. |
| `d` | The newest failure. | Dismisses it. |

A key acts only on a row that is on screen, so on a short terminal, a pull hidden under older rows is left alone.

### The footer

The footer lists the keys that always apply on the left, then what the selected model can do, with `?` help and `q` quit against the right edge. It is designed for 100 columns and up; narrower, it drops the pulls, sort and expand keys first, then the model's actions one at a time, then the core keys, so help and quit always show.

A short notice takes the footer over for two seconds when a key is refused or something finishes, for example `qwen3 is already warm` or `copied`.

### Under 100 columns

On a terminal narrower than 100 columns, the cards stack instead of sitting side by side, and the side margins go:

1. the shelf on top,
2. a compact model card of four rows (fit, residency, the last day's gateway traffic, and the path or size), titled with the model's name and its size,
3. the machine card, with the gateway's state folded in as a fourth line.

The compact model card shows only while the shelf keeps at least six rows, and the machine card only while the shelf keeps twelve (or all it needs, when it needs fewer). On a short terminal they drop from the bottom up rather than squeezing the shelf.

### Sizes on the shelf

The size column and the size sort follow what serving the model loads, as the FIT column of [`hedos ls`](cli.md#serving-size-and-disk-size) does. The model card's facts line adds the disk figure when it differs. The machine card's disk per store counts everything on disk with each file once, as `hedos scan` does. The removal card counts the model's own disk figure.

## Keys

### Moving

| Key | Does |
| --- | --- |
| `j` / `k`, `↑` / `↓`, the wheel | Move the selection. |
| `g` / `G`, `Home` / `End` | Jump to the top or the bottom. |
| `enter` | Expand the model card over the body, or collapse it. |
| `esc` | Collapse the card, or clear the filter. |

### The selected model

| Key | Does | Same as |
| --- | --- | --- |
| `w` | Warm it. | `hedos warm` |
| `u` | Unload it. | `hedos unload` |
| `x` | Remove it, after a card showing what leaves the disk. | `hedos rm` |
| `t` | Try it here: open the [try screen](#the-try-screen). | |
| `T` | Chat with it in the plain terminal. | `hedos chat` |
| `l` | Launch a coding harness on it. | `hedos launch` |
| `b` | Measure it on the [bench screen](#the-bench-screen). | `hedos bench <model>` |
| `y` | Copy its weights path. | |
| `Y` | Copy its id. | |

The footer offers a model's keys only when they apply: `w` when it can be warmed, `u` when it can be unloaded from here, `l`, `t` and `T` when it chats (`t` alone for a judge or an extractor), `x` when it can be removed, and `y` when it has a weights path. A key pressed anyway says why not in the footer, unless the model is busy with a task the strip already shows.

### The shelf as a whole

| Key | Does | Same as |
| --- | --- | --- |
| `p` | Open the [pull screen](#the-pull-screen). | `hedos pull` |
| `P` | Open the [pulls screen](#the-pulls-screen). | `hedos pull ls` |
| `B` | Open the [bench screen](#the-bench-screen). | `hedos bench` |
| `s` | Scan the machine's stores again. | `hedos scan` |
| `/` | Filter the shelf. | |
| `o` | Change the sort. | |
| `r` | Re-read the shelf and the machine facts now. | |
| `c` | Stop the newest pull still downloading. | `hedos pull pause` / `cancel` |
| `R` | Resume the newest stopped pull. | `hedos pull resume` |
| `d` | Dismiss the newest failed row from the strip. | |

### The screen

| Key | Does | Same as |
| --- | --- | --- |
| `S` | Serve the gateway in this terminal. | `hedos serve` |
| `?` | List every key. `esc`, `?` or `q` closes the list. | |
| `q`, Ctrl-C | Quit. | |

Ctrl-C quits from anywhere except the try screen, where it stops a reply or goes back to the shelf.

### Filtering and sorting

`/` opens the filter in the shelf card's title. Typing narrows the shelf to the models that match on their name, store, runtime, or a capability (a fuzzy, case-insensitive match), or whose id contains what you typed. The title shows how many rows are left (`/ qwen · 3 of 12`). `enter` keeps the filter and goes back to the shelf keys; `esc` clears it. The arrows and the wheel still move the selection while you type. A filter that matches nothing says so, with `esc` to clear it.

`o` cycles the sort: by name, by size (largest first, by what serving loads), by last used (most recently requested through the gateway first), and warm first. The sort keeps the selection on the same model.

### Editing text

Every text field (the try screen's box, the judge's fields, the pull search, the filter) edits like a shell line:

| Key | Does |
| --- | --- |
| `←` / `→` | Move a character. |
| Option+`←` / `→`, Alt-b / Alt-f | Move a word. |
| Ctrl-A / Ctrl-E | Jump to the start or the end. |
| Ctrl-U | Clear back to the start. |
| Ctrl-W, Option+Delete | Cut the word before the cursor. |
| Delete | Delete forward. |

Cmd+Delete is a macOS binding the terminal keeps to itself. In iTerm, map it to send Ctrl-U (hex `0x15`) if you want it here.

A paste arrives whole. In a one-line field its line breaks read as spaces, so a pasted text never sends or moves on by itself; an extractor's text keeps them. On the shelf itself, where letters are commands, a paste is ignored rather than run as keys.

## Acting on a model

### Warming and unloading

`w` warms the selected model where it is served. When a gateway is running and the model chats, it is loaded on the gateway; otherwise it is loaded by this process. The strip says which (`loading on the gateway`, `loading in this process`). Warming is refused, with the reason in the footer, for a model that is already warm, whose weights are gone, that is too big for the machine, or that has no warm request.

`u` unloads the selected model from this process, or through the Ollama daemon when the daemon holds it. A model the gateway holds cannot be unloaded from here; it unloads there after its warm window.

### Removing

`x` opens a card that shows exactly what leaves the disk: the store, the bytes on disk, the path, what happens (`deletes 3 paths permanently, not to the trash`, `removes the tag through the Ollama daemon · ollama rm`, or, for a record whose weights are already gone, `nothing is left on disk; this forgets the record`), and how much will be on disk after. `y` removes, `n` or `esc` keeps.

A warm model must be unloaded first. When you press `y`, the card checks again; if the model changed on disk while the card was open, it asks you to look again instead of deleting.

### Hand-offs: launch, chat, serve

`l`, `T` and `S` hand the terminal over. The UI steps aside, the command owns the terminal, and the shelf is back the moment it ends, with a row in the strip saying how it went (`ran 4m`, or `ran 4m · exit 1`).

- **`l`** opens a card of the harnesses (Claude Code, OpenCode, Aider, Goose, Crush). One that is not installed, or that needs tool calls from a model without them, is listed with the reason. `j`/`k` or the arrows move, `enter` launches, `esc` closes. See [`hedos launch`](cli.md#hedos-launch).
- **`T`** runs `hedos chat` on the model. Ctrl-C stops a reply; Ctrl-D ends the chat.
- **`S`** runs `hedos serve` on the configured port, unless a gateway is already up there. Ctrl-C stops it and brings the shelf back.

Ctrl-C reaches the harness or stops the reply; it never kills the shelf with pulls and state in flight.

### Copying

`y` copies the selected model's weights path and `Y` its id. The text goes two ways at once: through `pbcopy`, to the pasteboard of the machine hedos runs on, and by an OSC 52 escape, to the terminal you sit at. Over ssh, the second is the one that reaches your own clipboard. tmux relays OSC 52 only with `set -g set-clipboard on`.

## The try screen

`t` on a model that chats opens the try screen in the shelf's place: a conversation with the selected model, run in the shelf's own process. The transcript card is titled `try <model>`, with how many turns there have been on its right edge (`new` before the first).

### Talking to a model

- Your messages sit in bubbles on the right, with the time over them. Replies stream in on the left at reading width, under the model's name.
- A reply keeps its markdown: **bold**, headings, inline code, and fenced code on a darker panel with its language.
- Beside the reply's name, hedos shows how it is going: `loading into memory` while a cold model loads, `thinking` until the first token, the live speed while it streams, then its figures once it ends (`38 tokens · 42 tok/s · 1.3s`), or that it was stopped or failed. A `~` marks figures counted from the text because the runtime reported none.
- The whole conversation goes with each message.

Type into the box at the foot and press `enter` to send. Nothing is sent while the box is blank or a reply is still streaming.

### Suggestions

Before anything is asked, the transcript shows the model's name and facts and three things to ask:

```
TRY ASKING

› explain what you can do in two lines

  what is a kv cache, briefly?

  write a haiku about a koala on a shelf

tab puts one in the box
```

`tab` puts the marked suggestion in the box, and pressing it again moves to the next one. It never replaces something you typed yourself.

### Scrolling

| Key | Scrolls |
| --- | --- |
| `↑` / `↓` | A line. |
| The wheel | Three lines a notch. |
| `PageUp` / `PageDown` | Ten lines. |
| `Home` / `End` | To the top, or back to the newest text. |

A scrolled view holds still while more text streams in. Its title shows where you are (`line 40 of 212`), and a note at its foot counts the newer lines waiting under it (`↓ 18 newer lines`). `End` follows the newest text again.

### Stopping and leaving

- While a reply streams, `esc` or Ctrl-C stops it. What streamed so far stands.
- While idle, `esc` or Ctrl-C goes back to the shelf.
- `⌃l` (Ctrl-L) starts the conversation over with the same model. It is refused while a reply streams.

The model is warm afterwards, as after `hedos run`.

A bench and a conversation would each skew the other's figures, so `t` is refused while a bench runs (`press B, then c to stop it`).

### The session card

On a terminal at least 100 columns wide, a session card sits beside the conversation:

- the model, its runtime and size, and whether it is held (`● warm · unloads in 4m`, or `○ cold · loads on send`),
- **CONTEXT**: how much of the model's context window the conversation fills, as a bar and a count (`1.2k of 32k tokens`), counted by the runtime when it reports it and estimated (with a `~`) otherwise,
- **LAST REPLY**: the last reply's speed in big figures (`tok/s`), its token count and time, and how long the first token took,
- **SPEED**: a bar per reply and the average,
- **SESSION**: how many turns since when, and `in this process`,
- `⌃l clear` and `esc shelf` (or `esc stop` while a reply streams) at its foot.

On a narrower terminal there is no session card; the transcript's title says `warm` or `cold` instead.

### Kept conversations

Leaving an idle conversation keeps it, along with anything typed and not yet sent. `t` on the same model takes it up where you left it. Only the last conversation is kept: opening the try screen on another model lets it go.

## Judges on the try screen

A judge (a decision model such as laya, clef or OpenJev) answers typed questions rather than prose. `t` on a judge opens the same screen, titled `judge <model>`, with a composer for typed questions in the box's place.

### The kinds of question

| Kind | The model | You give it |
| --- | --- | --- |
| `choice` | picks one of your options | A question and at least two options. |
| `score` | rates against your levels | A question and at least two levels, lowest first. |
| `noul` | weighs how far a statement holds | A statement. |

### The fields

| Field | What goes in it |
| --- | --- |
| kind | `choice`, `score` or `noul`. |
| situation | What is being judged (a ticket, a log line, a message). Optional: leave it empty when the question stands on its own. |
| question | What should be decided about it. For a `noul` it is called the statement. |
| options / levels | A choice's options or a score's levels, added one at a time. Not shown for a `noul`. |

An option is typed as `label: what it means`, or just a label. Two options cannot share a label.

### Keys

| Key | Does |
| --- | --- |
| `tab` / `shift-tab` | Move to the next or previous field. |
| `←` / `→`, space | Change the kind, on the kind field. |
| `enter` | On the kind or the situation, move on. On the question, move to the options, or ask, for a `noul`. On the options, add the option typed, or ask once nothing is being typed. |
| `backspace` | On an empty option field, take the last option back to edit. |
| `⌃l` | Start over. |
| `esc`, Ctrl-C | Stop the judgment in progress, or go back to the shelf. |

The transcript scrolls with the same keys as a conversation. The composer's corner shows the key that does something in the field you are on (`tab next`, `enter add`, `enter judge`).

### The answer

Each answer is drawn as its distribution: a bar and a share for each outcome, with the model's answer marked and bright.

```
▌ refund     ━━━━━━━━━━━━━━━━━━━━━━━━━━            71.2%
  money back
  replace    ━━━━━━━                               20.0%
  apologize  ━━━                                    8.8%

  confidence 80.0%
```

On screen, the rest of each bar's track is drawn as a dim line, and the bars widen with the card.

- A **choice** lists its options, likeliest first, with the chosen option's meaning under it.
- A **score** lists its levels from 0 up, marks the likeliest, and adds the expected level (`expected 2.4 of 4`).
- A **noul** shows `yes` and `no`.
- When the model says how sure it is, `confidence` follows.

The session card shows the chosen outcome's share in big figures (`%`), its label, and how long it took (`judged in 0.4s`), with `each ask stands alone` under SESSION. Each ask empties the fields, keeping the kind, and stands alone: nothing from an earlier ask goes with the next one.

## Extractors on the try screen

An extractor such as [Tessera](models.md#extractors) finds contacts in text. `t` on one opens the same screen, titled `extract <model>`, with a composer for the text in the box's place.

| Field | What goes in it |
| --- | --- |
| operation | `detect` (every entity found), `contacts` (grouped by who they belong to), or `address` (one address split into its parts). |
| text | The text to read. A pasted text keeps its line breaks and tabs, and the field grows to four rows to show it. |

`tab` moves between the two fields, `←` / `→` or space change the operation, and `enter` on the text reads it. Each read empties the text, keeping the operation, and stands alone.

The answer lists what was found, one row per entity: its kind, its text, a bar and a figure for how sure the extractor is, and under it the canonical form of a phone or email when that differs from the text. Contacts are headed by the person or organization they belong to, then come the entities in no contact; an address is its parts, one per row. A figure the extractor suggests a person check, or one under 0.5, is drawn in the warning colour.

The session card shows how many things the last read found in big figures, what they were (`1 contact · 5 entities`), and how long it took (`read in 40ms`). An extractor holds nothing in memory between reads, so the card says it `runs once per request` where a model's residency would be, and `w` does not warm it.

## The pull screen

`p` opens the pull screen in the shelf's place: a search over the catalog and Hugging Face, the results, a preview of the row under the cursor with the one button that pulls it, and the downloads in flight.

### Searching

The field at the top takes a name, an `owner/repo`, or a `name:tag`. Beside it, each source shows its state: the catalog is always there (`✓ catalog`), and Hugging Face is searched once the query sits still (a spinner, then `✓`, or `✕` if it could not be asked).

The results combine:

- a row for exactly what you typed, when it is a full `owner/repo` or `name:tag` (noted `as typed`),
- the catalog's recommendations that fit this machine's memory, narrowed by the query; with nothing typed, on the `all` kind, they are grouped under CHAT, CODE, SPEECH and IMAGE,
- Hugging Face hits, with their download and like counts.

The list holds up to twelve matches, and Hugging Face hits always keep their places.

### Kinds

Under the field, chips choose a kind: `all`, `chat`, `code`, `speech`, `image`, each with its count. `tab` and `shift-tab` step through them. The result count sits on the right.

### The results

The results card says what it lists (`recommended for 64 GiB`, `chat models`, or `matching “qwen”`). Each row shows:

- a mark: `●` already on the shelf, `✕` on the shelf but its weights are gone, `○` not on the shelf, or a spinner while it downloads,
- the reference, where it comes from (`ollama` or `hf`), and its size,
- a gauge of how much of the machine's memory it takes, with `fits`, `tight` or `too big`, `pull again` for a model whose weights are gone, `✓ on shelf`, or a download's percentage,
- the hub's download count, on a wide card.

### The preview

The preview follows the cursor. It sits beside the results from 100 columns, and under them on a narrower terminal. It shows:

- the owner or registry, the name, and its kind and size, with the catalog's note about it,
- **FIT**: the verdict, what it needs of the machine's memory, and a gauge of what is loaded, what this model would take, and what would be left,
- **FILES** (Hugging Face) or **LAYERS** (Ollama): the plan, once it has come back. That is the files or layers it fetches with their sizes, where they land (`to`), and, when some of it is already on disk, how much is left to get.

A plan is asked for each row the cursor rests on, not for the rows you scroll past (`planned once the cursor rests here`, then `planning`). A plan that fails is asked again only once you move away and come back. A gated repository says so and needs a Hugging Face token first.

### Pulling with one enter

`enter` pulls the selected model. Pressed before its plan has come back, the pull starts as soon as it does; moving off the row first lets that go. Nothing moves until you press `enter`.

`enter` is refused, with the reason, for a model:

- already on the shelf (one whose weights are gone is offered as "pull again"),
- already downloading,
- too big for this machine (the preview suggests that a smaller quantization of it may fit),
- that is gated, or whose plan failed.

The download runs in a worker of its own and shows in the downloads card and on the preview's button (`pulling 42% · 1.9 GB of 4.6 GB`), so you can start several and keep browsing. It runs on if you quit. When it lands, the shelf selects the new model.

### Downloads

The downloads card under the results lists the pulls in flight with their progress, or says `nothing downloading · a pull runs on after you quit`.

### Keys

| Key | Does |
| --- | --- |
| typing | Search. |
| `↑` / `↓`, the wheel | Move. |
| `PageUp` / `PageDown` | Move ten rows. |
| `Home` / `End` | Jump to the first or last row. |
| `tab` / `shift-tab` | Change the kind. |
| `enter` | Pull the selected model. |
| `esc` | Clear what was typed, then go back. |

Letters type into the field here, so `q` does not quit. Ctrl-C still does.

## Stopping and resuming pulls

`c` on the shelf, or on the pulls screen, opens the stop card over a pull. Nothing stops until you answer it:

```
 model    Qwen/Qwen2.5-1.5B-Instruct
 on disk  3 GB of 4 GB

 pause keeps what has landed, to resume later
 cancel ends it for good; the pull cannot be resumed

 p pause  x cancel  esc keep going
```

- `p` pauses: the bytes stay, and `R` resumes it later.
- `x` cancels for good. Only a fresh pull of the same model, within `pull.partial_age_hours`, finds the bytes again. Cancel answers to `x` and not to `c`, so a held `c` can never cancel through the card.
- `esc` or `n` leaves it going.

The card follows its pull while it is open, and closes with a note if the pull lands or stops without it. A pull that is past the point a stop can reach (it is being registered) cannot be stopped, and the footer says how many seconds that has left.

`R` resumes the newest stopped pull from the shelf, or the selected one from the pulls screen. See [`hedos pull`](cli.md#pausing-and-cancelling) for how pauses and cancels settle.

## The pulls screen

`P` opens the pulls screen in the shelf's place: every pull the store still holds, newest first, with REFERENCE, STATE and PROGRESS. It holds everything until `hedos pull clean` takes it, while the task strip keeps only the newest work.

Beside the list, the selected pull's detail shows its state (and attempt), progress with a bar, its rate and an estimate of the time left, its note, its id, where it comes from and where it lands, when it started and was last updated, and its HISTORY, newest last. The rate is the screen's own reading of the progress the worker writes, so it starts when the screen opens.

| Key | Does |
| --- | --- |
| `j` / `k`, `↑` / `↓`, `g` / `G` | Move. |
| `c` | Stop the selected pull, through the stop card. |
| `R` | Resume the selected pull. |
| `Y` | Copy its job id, which `hedos pull` commands take. |
| `x` | Forget an ended pull's record, which is what `hedos pull clean` does to all of them. |
| `p` | Start a new pull, as on the shelf. |
| `esc`, `P` | Back to the shelf. |
| `?`, `q` | Help, quit. |

An interrupted, superseded or unreadable pull reads here as it does in [`hedos pull ls`](cli.md#interrupted-superseded-and-unreadable-pulls).

## The bench screen

`B` opens the bench screen in the shelf's place: every chat model that fits the machine, measured one at a time, with the selected row's figures beside the list. Opening it with no bench yet starts one over the whole shelf. `b` on the shelf benches just the model under the cursor, whatever its size, and opens the screen on it.

The rows and figures are the ones [`hedos bench`](cli.md#hedos-bench) reports, from the same driver. The detail shows the selected row's state, rate, spread, first token, cold start, the prompt, where its timing came from (the runtime's own, or the wall clock), and each run. The selection follows the row being measured until you move it.

| Key | Does |
| --- | --- |
| `j` / `k`, `↑` / `↓`, `g` / `G` | Move. |
| `a` | Bench every model that fits, in a fresh table. |
| `b` | Measure the selected row again, keeping the others to compare against. |
| `c` | Stop the bench; what it has measured stands. |
| `esc`, `B` | Back to the shelf. The bench carries on. |
| `?`, `q` | Help, quit. |

Only one bench runs at a time, and `t` is refused while one runs.

## Motion

The screen moves to mark a change: a spinner while you wait, a figure easing to its new value, a name arriving letter by letter, a card opening, a download's bar shimmering. The pulse's bars rise once when the screen opens. It also keeps a few quiet signs of life: the koala sways and breathes in the hero, the gateway's dot pulses while it serves, and warm models' dots breathe.

`HEDOS_MOTION` changes that:

| Value | Effect |
| --- | --- |
| `off` (or `0`, `false`, `no`) | Stops all of it. Every frame is a final one, and spinners turn on the quarter-second tick instead. Elapsed times still count. |
| `slow` | Plays every movement ten times slower, to look at one closely. |
| anything else, or unset | Normal motion. |

```sh
HEDOS_MOTION=off hedos shelf
```

## Colour

The UI paints no ground of its own: every cell keeps your terminal's background, and a card is its border. Only a few things are filled: the selected row, a chip, your words and code in the try screen, a button. hedos asks the terminal for its background once when the screen opens, and its lightness says whether the terminal is light or dark. On a dark background lighter than near black, the colours are laid between your background and white, so a border, a label or a fill stands as far off it as it does off near black; on a darker one they stay as they are. On a light terminal every colour turns its lightness over, so ink reads dark and the hierarchy and the state colours stay the same.

A terminal that does not answer (`screen`, mosh, an old tmux) gets the UI's own near-black ground painted under everything, so the screen reads whatever its colours are. `HEDOS_THEME` says which kind of background it is instead:

| Value | Effect |
| --- | --- |
| `light` | Draw on a light background, even when the terminal does not answer. |
| `dark` | Draw on a dark background, even when the terminal does not answer. |
| anything else, or unset | Go by the terminal's answer. |

hedos still asks for the exact colour when `HEDOS_THEME` is set. When the terminal does not say it, the colours are the ones chosen for near black (or, turned over, near white).

```sh
HEDOS_THEME=light hedos shelf
```

It draws in 24-bit colour when `COLORTERM` is `truecolor` or `24bit`, the only reliable sign of more than 256 colours. With any other value, or none, every colour is mapped to the nearest one in the 256-colour palette.

If your terminal shows 24-bit colour but the variable does not reach hedos (ssh, for one, does not pass it on by default), set it yourself:

```sh
COLORTERM=truecolor hedos shelf
```

## Over ssh and in tmux

The shelf needs nothing but a terminal, so it runs the same over ssh and inside tmux.

- **The wheel** works through mouse reporting, which the shelf turns on while it runs and off when it leaves.
- **Copying** with `y` and `Y` reaches the clipboard of the terminal you sit at by OSC 52. tmux relays it only with `set -g set-clipboard on`. See [Copying](#copying).
- **Colour** may need `COLORTERM` set by hand; see [Colour](#colour).
- **Pulls** run in workers of their own and outlive the terminal that started them. One whose worker died anyway (a closed laptop, a killed session) is started again the next time the shelf opens, when `pull.auto_resume` is on.

## Quitting

`q` or Ctrl-C quits, and so does a closed terminal or a SIGTERM. The shelf saves its state and restores the terminal on the way out. A removal or scan that has started is finished first (`finishing background work…`). A download is never waited for: it belongs to a worker that outlives the screen.
