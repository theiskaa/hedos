import argparse
import os

os.environ.setdefault("HF_HUB_OFFLINE", "1")

import numpy as np
import soundfile as sf
from kokoro import KModel, KPipeline


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--model", required=True)
    parser.add_argument("--text", required=True)
    parser.add_argument("--voice", default="af_heart")
    parser.add_argument("--out", required=True)
    args = parser.parse_args()

    config = os.path.join(args.model, "config.json")
    weights = os.path.join(args.model, "kokoro-v1_0.pth")
    model = KModel(config=config, model=weights)
    pipeline = KPipeline(lang_code="a", model=model)

    voice = os.path.join(args.model, "voices", args.voice + ".pt")
    if not os.path.exists(voice):
        candidates = sorted(os.listdir(os.path.join(args.model, "voices")))
        if not candidates:
            raise SystemExit("no voices ship with this model")
        voice = os.path.join(args.model, "voices", candidates[0])

    chunks = []
    for _, _, audio in pipeline(args.text, voice=voice):
        chunks.append(np.asarray(audio))
    if not chunks:
        raise SystemExit("the pipeline produced no audio")
    wav = np.concatenate(chunks)
    destination = os.path.join(args.out, "speech.wav")
    sf.write(destination, wav, 24000)
    print(f"wrote {len(wav) / 24000:.1f}s of audio to {destination}")


if __name__ == "__main__":
    main()
