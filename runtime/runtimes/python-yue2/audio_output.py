"""How an audio runtime hands hedos a finished piece: the file formats it
writes, their encoding, and the `result` event followed by the file in frames.

Every audio runtime bundle carries this file byte for byte (a test holds them
to it): a runtime is approved on its own files, so it cannot import another
bundle's.
"""

import io
import json

# A frame may carry at most 16 MiB; a song is sent as several.
RESULT_CHUNK = 8 * 1024 * 1024

FORMATS = ("flac", "wav")


class FormatError(ValueError):
    """A file format no audio runtime writes: the request's fault."""


def output_format(request):
    fmt = str(request.get("format") or "flac").lower()
    if fmt not in FORMATS:
        raise FormatError(f"audio comes as {' or '.join(FORMATS)}, not {fmt}")
    return fmt


def encode(samples, sample_rate, fmt):
    """`samples` (frames by channels, or one channel) as a `fmt` file's bytes:
    24-bit FLAC, or float WAV."""
    import numpy as np
    import soundfile

    audio = np.clip(np.asarray(samples, dtype=np.float32), -1.0, 1.0)
    buffer = io.BytesIO()
    subtype = "PCM_24" if fmt == "flac" else "FLOAT"
    soundfile.write(buffer, audio, int(sample_rate), format=fmt.upper(), subtype=subtype)
    return buffer.getvalue()


def send_result(send, data, fmt, sample_rate, channels, seconds, chunk=RESULT_CHUNK):
    """Announce the finished file with its size through `send`, the runtime's
    frame writer, then send the file in frames of at most `chunk` bytes."""
    announcement = {
        "event": "result",
        "format": fmt,
        "bytes": len(data),
        "sample_rate": int(sample_rate),
        "channels": int(channels),
        "seconds": round(float(seconds), 3),
    }
    send(1, json.dumps(announcement).encode())
    for start in range(0, len(data), chunk):
        send(2, data[start : start + chunk])
