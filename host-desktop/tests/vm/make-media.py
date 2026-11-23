# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
"""The pair B media: tone.wav (30 s, 440 Hz at -10 dBFS, 48 kHz stereo; its RMS is
0.2236) and clip.png (64x48 RGB), written into the directory given."""
import math
import struct
import sys
import wave
import zlib

out = sys.argv[1] if len(sys.argv) > 1 else "."

with wave.open(f"{out}/tone.wav", "wb") as w:
    w.setnchannels(2)
    w.setsampwidth(2)
    w.setframerate(48000)
    a = int(0.316 * 32767)
    frames = bytearray()
    for i in range(48000 * 30):
        v = int(a * math.sin(2 * math.pi * 440 * i / 48000))
        frames += struct.pack("<hh", v, v)
    w.writeframes(bytes(frames))

W, H = 64, 48
raw = b"".join(b"\x00" + bytes(c for x in range(W) for c in (x * 4, y * 5, 128)) for y in range(H))


def chunk(t, d):
    return struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d) & 0xFFFFFFFF)


png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", W, H, 8, 2, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b"")
open(f"{out}/clip.png", "wb").write(png)
