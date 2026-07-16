# Configuring a model

Every model on your shelf carries its own configuration — a system prompt, a set of generation parameters, and a display name. None of it is global; it travels with that one model and is remembered between launches. Customization is the point of Hedos, and this is where most of it happens.

## Where configuration lives

Open a model from the shelf and its detail sheet appears. Scroll to the **Configure** section: that is the whole surface. A chat-capable model shows a **System prompt** field at the top, then a row for each parameter the model's runtime actually honors, and a **Reset** control at the bottom. A model with no chat capability and no parameters shows nothing here — there would be nothing true to put.

Everything you change saves on its own. There is no save button; edits are written to the model a moment after you stop typing or dragging.

## The system prompt

The system prompt is a standing instruction prepended to every conversation with this model. Type it into the **System prompt** field — the helper line reads "Prepended to every conversation with this model." Leave it empty to have none.

At the start of each conversation Hedos injects the prompt as a `system` turn, ahead of your first message. Two honest details:

- If a conversation already carries its own system turn, Hedos does not add a second one — the conversation's own instruction wins.
- The prompt applies to chat. Non-chat invocations (transcription, speech, embeddings) don't take one, so it isn't offered there.

Clearing the field removes the prompt entirely; whitespace-only text counts as empty.

## Per-model parameters

Below the prompt sits one row per parameter. Every chat or completion model exposes **Temperature** (0–2), **Top P** (0–1), and **Max Tokens**. Which further knobs appear depends on the runtime bound to the model:

- **llama-cpp** adds Top K, Min P, Repeat Penalty, Frequency Penalty, Presence Penalty, Seed, and Stop.
- **ollama** adds Top K, Min P, Seed, Repeat Penalty, Frequency Penalty, Presence Penalty, and Stop.
- **mlx-lm** adds Top K, Min P, Repeat Penalty, Seed, and Stop.
- **mlx-swift** adds Repeat Penalty and Stop.
- **apple-foundation** adds Top K and Seed.

Speech models show **Voice** and **Speed** instead; transcription models show **Language** and **Translate**; and chat models on ollama or mlx-lm gain a **Thinking** toggle. The set is never padded with knobs the engine can't use.

Each row starts at **Auto** — the model decides, and Hedos sends nothing for that knob. Set a value and a small filled dot appears next to the label to mark it as overridden, along with an ✕ to clear that one override back to Auto. The footer states this plainly: "Overrides apply to the next generation. Auto means the model decides."

Overrides persist per model. They are attached to that specific model record, so two models built from the same weights keep separate settings.

### Resetting

The **Reset** button at the bottom of the section clears every override at once, returning all rows to Auto. It is disabled when there is nothing to reset. To drop a single knob instead, use the ✕ on its row.

## Context length

For chat and completion models served by **ollama** or **llama-cpp**, a **Context Length** row appears. It sets the working context window — how many tokens of prompt plus reply the model keeps in view for a turn. The slider runs from a small floor up to the model's trained window (Hedos reads that from the model where it can, and starts you at a sensible default below the ceiling rather than at the maximum).

This knob is deliberately absent on other runtimes. The mlx runtimes run at the model's own fixed window and offer no dial to move it, so Hedos does not show one — a control that did nothing would be a lie. Apple Foundation likewise manages its own window. If you don't see a Context Length row, the runtime serving that model doesn't let it be set.

## The honest boundary

The reason the knob set changes from model to model is a single rule: **Hedos passes a parameter only to an engine that supports it, and refuses it loudly everywhere else** rather than quietly dropping it and pretending it took effect.

In the app this shows up as the Configure section only ever listing knobs the current runtime honors. Switch a model's runtime and the list changes to match — a value you set for a knob the new runtime lacks simply won't be sent, because that runtime never advertised it. Over the local gateway the same rule has teeth: send an unsupported parameter in an API request and you get a `400` with an `unsupported_parameter` error naming the offending key, not a silent success.

So the same dial can be live on one runtime and rejected on another, and that is the intended behavior. It means a value you see applied is a value that was genuinely honored.

## Renaming a model

The title at the top of the detail sheet is the display name, and it is editable. Hover it and a pencil appears; click to edit inline, then press Return to commit or Escape to cancel.

The name you type is stored as an alias — the underlying model keeps its original name untouched. Clear the field (or type the raw name back exactly) and the alias is removed, so the title reverts to the model's real name. When a display name differs from the raw name, the detail sheet also shows the original under the model's specification, so the true identity is never hidden.
