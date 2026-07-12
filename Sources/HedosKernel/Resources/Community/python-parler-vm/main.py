import argparse
import os

os.environ.setdefault("HF_HUB_OFFLINE", "1")

import soundfile as sf
import torch
from parler_tts import ParlerTTSForConditionalGeneration
from transformers import AutoTokenizer


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--model", required=True)
    parser.add_argument("--text", required=True)
    parser.add_argument(
        "--description", default="A clear, natural voice at a steady pace.")
    parser.add_argument("--out", required=True)
    args = parser.parse_args()

    model = ParlerTTSForConditionalGeneration.from_pretrained(args.model)
    tokenizer = AutoTokenizer.from_pretrained(args.model)

    prompt = tokenizer(args.text, return_tensors="pt").input_ids
    description = tokenizer(args.description, return_tensors="pt").input_ids
    with torch.no_grad():
        audio = model.generate(input_ids=description, prompt_input_ids=prompt)
    wav = audio.cpu().numpy().squeeze()
    destination = os.path.join(args.out, "speech.wav")
    sf.write(destination, wav, model.config.sampling_rate)
    print(f"wrote audio to {destination}")


if __name__ == "__main__":
    main()
