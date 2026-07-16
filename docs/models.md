# Installing and managing models

Your shelf is not read-only. Hedos discovers the models already on your Mac, but it also installs new ones — from the app or from the terminal — and removes them again. The guiding principle is that Hedos owns no weights directory of its own: an Ollama model is pulled through the Ollama daemon into Ollama's store, and a Hugging Face model lands in the standard hub cache. Every other tool on your machine sees the same files. Nothing is hidden in a private folder, and nothing is moved.

## The install browser

Open the Models screen and start an install to get the browser. It has three ways in, and they all end at the same confirm page.

**The curated catalog.** Under "Recommended for your Mac" is a short list of models chosen to fit your hardware — sized so they will actually run on the RAM you have. Tabs across the top group them by what they do: chat, code, voice, and image. This is the fastest path if you just want something good that works.

**Searching Hugging Face.** The search field ("Search Hugging Face, or paste gemma3:4b / org/repo") queries the Hugging Face hub live. Type a name and the catalog is replaced by "Hugging Face results" — pick one to review it. If nothing matches, the browser says so and suggests pasting an exact `org/repo` instead.

**Pasting a link or reference.** The same field accepts an exact reference and recognizes what it is:

- a `huggingface.co/…` or `ollama.com/…` link
- an `org/repo` (goes to Hugging Face)
- a `name:tag` such as `gemma3:4b` (goes to Ollama)

When you paste an Ollama-style tag, a "Review …" row appears directly beneath the field — click it to jump straight to the confirm page for that tag, skipping search entirely.

## The confirm page: what lands on disk

Nothing downloads until you have seen the plan. Before a single byte moves, Hedos resolves the reference and shows you exactly what the install will do.

At the top, a stats strip: the total **download size**, the **file count** (broken into weights versus configuration files), and the **source** — including the pinned revision the download is fixed to, shown as a short commit hash for Hugging Face models, or "through the local daemon" for Ollama.

Below that, "What downloads" lists every file by name with its individual size, the weight files marked. "Lands in" names the destination on disk — the same hub cache Hugging Face tooling reads.

An Ollama pull works differently and the page says so plainly: there is no per-file list because the daemon pulls the tag layer by layer, exactly as `ollama pull` would, straight into Ollama's own store. Layer sizes and progress appear the moment the transfer starts.

Press **Install** to begin. You can watch progress under "Downloading now" and keep browsing while it runs.

## Where installs go, and why it matters

Hedos writes each model into its platform's native habitat rather than a directory of its own:

- **Ollama models** are pulled through the local Ollama daemon's own API. They land in Ollama's store where `ollama` and every other Ollama-aware tool can use them. The scanner watches that store, so the model appears on your shelf without a manual rescan.
- **Hugging Face models** download into the standard hub cache layout (blobs, snapshots, refs) — the same cache `huggingface-cli`, `transformers`, and friends already read.

Two consequences worth knowing. First, an interrupted download resumes: substantial progress is kept, and starting the same install again picks up where it left off rather than beginning over. Second, Hugging Face files are checksum-verified as they arrive — each blob is hashed and checked against the revision it was pinned to, so a corrupted or truncated download is caught rather than silently trusted.

## Gated Hugging Face repositories

Some Hugging Face repos require you to accept the owner's terms and present an access token before the files will download. When a plan needs one, the confirm page marks the model gated and refuses to start until a token is available — this is a deliberate stop, not a failure.

Set a token in **Settings → Models**, under "Hugging Face access token". Paste an `hf_…` token and save it. The token is stored in your macOS keychain and never written to a settings file on disk. Once it is there, gated models download with it automatically.

You may not need to add it there at all. If you already have a token in the `HF_TOKEN` environment variable, or you have run `huggingface-cli login`, Hedos uses that too — no separate entry required. If a gated install still stops after you have set a token, use "Check again" on the confirm page to re-plan with the new credential.

## Installing from the terminal

The `hedos` command drives the same install service headlessly, so it works over SSH and in scripts. Fetch a model with `pull`:

```sh
hedos pull qwen2.5:3b            # a name:tag goes to Ollama
hedos pull mlx-community/Llama-3.2-3B-Instruct-4bit   # an org/repo goes to Hugging Face
```

Without an argument, `hedos pull` prints models recommended for this Mac's RAM instead of fetching anything:

```sh
hedos pull
```

While a pull runs, progress reports the bytes downloaded (against the total, with a percentage, once the size is known) and the file currently transferring. Press **Ctrl-C** to cancel — any substantial progress stays in the store, and running the same pull again resumes from it.

By default Hedos infers the source: an `org/repo` goes to Hugging Face, everything else to Ollama. Force it either way with `--from`:

```sh
hedos pull some-name --from huggingface
hedos pull user/model:latest --from ollama    # an Ollama community model
```

A gated Hugging Face repo will stop with a message telling you to set `HF_TOKEN` or run `huggingface-cli login` first, then retry.

For scripting, `--json` (a global flag, so it goes before or after the subcommand) prints a machine-readable result instead of the human progress line:

```sh
hedos pull qwen2.5:3b --json
```

## Deleting models

Removal is symmetric with install: you always see what would go before anything moves, and file-backed deletions are reversible.

### From the app

Open a model's detail sheet on the Models screen. The delete affordance is a quiet trash icon that warms when you hover it — there are no red rows waiting to be clicked by accident. Selecting it raises a confirmation that tells you the truth about what will happen:

- For a **file-backed model**, it names how many items move to the **Trash** and how many bytes that frees. Because the files go to the Trash rather than being erased, you can put them back. If another model on your shelf points at the same files, the dialog says so — "Copies stay at: …" — so you know the weights are not really gone.
- For an **Ollama model**, it asks the Ollama daemon to delete the tag, up to the size shown. Layers shared with other Ollama models stay behind; that is how Ollama's store works, not a leak.
- If the files are **already missing**, deleting simply forgets the stale entry.

### From the terminal

`hedos rm` is a dry run by default — it prints what deleting would remove and changes nothing:

```sh
hedos rm gemma3
```

That shows the model, its kind, the estimated bytes, and either the exact paths that would move to the Trash or the note that it would ask the Ollama daemon. To actually delete, add `--yes` (or `-y`):

```sh
hedos rm gemma3 --yes
```

File-backed models go to the Trash, Ollama models delete through the daemon — the same behavior as the app. `--json` works here too, on both the dry run and the real deletion.
