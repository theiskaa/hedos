# Changelog

## v0.1.3 - 2026-07-16

- Install models from inside hedos: the Models screen gains an Install browser with a curated catalog fitted to the Mac's hardware, Hugging Face search, and direct entry — paste a `huggingface.co` or `ollama.com` link, an `org/repo`, or a `name:tag` and it resolves. A confirm page shows exactly what will land on disk (size, file manifest, destination, pinned revision) before anything downloads.
- Ollama installs drive the local daemon's own pull (auto-starting it when needed); Hugging Face installs download straight into the standard hub cache layout with resume after interruption and per-file checksum verification. Hedos still owns no weights directory — everything lands where the native tools expect it, so other tools see the models too.
- Live download surfaces everywhere: determinate progress on model cards, a full-width banner on the Models dashboard that flips into aggregate progress while downloads run, cancel that keeps resumable progress, and failure rows that announce a dead install instead of letting it vanish. First-run suggestion cards install for real.
- `hedos pull` installs from the command line: byte progress on a TTY, Ctrl-C cancels with a resume hint, `--from ollama|huggingface` disambiguates references, `--json` emits a final report. Links work there too.
- Hugging Face access token gets a home in Settings > Models — keychain-backed, never written to a settings file. Gated repos point there, and "Check again" unlocks Install without leaving the page. `HF_TOKEN` and existing `huggingface-cli` logins keep working.
- Delete models: the model detail sheet gains a trash button (⌘⌫) with a confirmation that says exactly what would go — item count, bytes, and where duplicate copies remain. File-backed models move to the Trash (reversible); Ollama models delete through the daemon. `hedos rm <model>` is a dry-run by default and deletes with `--yes`.
- Model detail sheet redesigned: the title renames inline like chat records (the separate display-name field is gone), sections adopt the settings-grade layout, and the sheet opens with a single clean entrance motion.
- Updater fixes: running from a mounted DMG or a Gatekeeper-translocated copy now installs updates to /Applications instead of punting to a manual drag; quitting for an update no longer hangs; the staged install re-verifies the code signature before swapping and relaunches the old copy if the swap fails.
- The home activity graph renders its full year grid from day zero instead of swapping itself for a "No chats yet" caption.
- Cmd+W while settings is open closes the settings panel (or the command palette above it) instead of the whole window, and Esc throughout the install browser steps back contextually instead of closing the modal.
- Fixed panels rounding their interior corners away from dividers on macOS 26.

## v0.1.2 - 2026-07-15

- Fixed the launch crash: v0.1.1 crashed on startup on every Mac except the machine that built it, because the packaged app looked for its bundled resources through a path that only existed there. Resources now resolve from inside the app bundle for both the app and the `hedos` command-line tool, including when the tool is run through its `/usr/local/bin` link.
- Text files attach to any chat model: the composer paperclip now accepts notes, code, CSV — any text file — regardless of the model, and the size limit adapts to the model's context window. Images still require a vision-capable model.
- Images paste and drag straight into the composer, including images dragged from a browser. Clipboards that carry both text and an image paste as text.
- Copying a chat image puts the full-resolution image on the clipboard instead of the display thumbnail, and Copy is available from the gallery and the lightbox.
- Settings opens as a panel inside the window instead of a separate one, with a redesigned appearance section (live theme previews for light, dark, and system), larger consistent controls, and Esc reliably closing the topmost thing.
- Assorted fixes: dropping non-WAV audio gets the correct message, attachment-size notices name the model's actual limit, and long values in model details wrap on hover instead of escaping the card.

## v0.1.1 - 2026-07-14

- In-app updater backed by GitHub Releases: the app checks for a newer version, downloads the DMG, verifies its checksum, and swaps itself in place. The sidebar version label doubles as the update button.
- Image attachments in chat: vision-capable models get a paperclip in the composer, and attached images travel with the conversation as content-addressed references.
- Restructured chat composer controls; the input keeps focus across mode switches and sends.
- Motion polish across the app: ad-hoc animation timings unified on the shared design tokens, modals and palettes dismiss faster than they open, and list stagger tightened so the last item never lands late.
- Image and voice model selection no longer drifts to the wrong record, and composer focus survives model changes.
- The Homebrew cask declares the macOS 26 (Tahoe) requirement with the modern `depends_on macos` syntax.
- The DMG background palette matches the website.

## v0.1.0 - 2026-07-14

The first release.

- Discovery of every local model already on the Mac: Ollama, the Hugging Face cache, LM Studio, loose GGUF and safetensors files, and the model Apple ships with macOS. Weights are never moved or re-downloaded; records point at where other tools put them.
- Chat, voice conversation, and image generation over the runtime that fits each model, with per-model parameter controls.
- A local gateway speaking the OpenAI and Ollama wire formats, so existing clients can talk to any discovered model.
- The `hedos` command-line tool, installed on the `PATH` by the app.
- Signed and notarized DMG for Apple Silicon, plus a Homebrew cask (`brew install --cask theiskaa/tap/hedos`).
