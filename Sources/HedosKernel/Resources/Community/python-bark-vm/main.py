import argparse
import os

os.environ.setdefault("HF_HUB_OFFLINE", "1")

import soundfile as sf
from bark import SAMPLE_RATE, generate_audio, preload_models


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--model", required=True)
    parser.add_argument("--text", required=True)
    parser.add_argument("--voice", default="v2/en_speaker_6")
    parser.add_argument("--out", required=True)
    args = parser.parse_args()

    os.environ.setdefault("XDG_CACHE_HOME", args.model)
    preload_models()
    wav = generate_audio(args.text, history_prompt=args.voice)
    destination = os.path.join(args.out, "speech.wav")
    sf.write(destination, wav, SAMPLE_RATE)
    print(f"wrote {len(wav) / SAMPLE_RATE:.1f}s of audio to {destination}")


if __name__ == "__main__":
    main()
