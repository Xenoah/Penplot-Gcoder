"""Generate original synthetic PNG fixtures with only the Python standard library."""
import struct
import zlib
from pathlib import Path


def png(path, w, h, pixels):
    def chunk(kind, data):
        return struct.pack('>I', len(data)) + kind + data + struct.pack('>I', zlib.crc32(kind + data))
    raw = b''.join(b'\0' + bytes(pixels[y*w*4:(y+1)*w*4]) for y in range(h))
    Path(path).write_bytes(b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', w, h, 8, 6, 0, 0, 0)) + chunk(b'IDAT', zlib.compress(raw, 9)) + chunk(b'IEND', b''))


pixels = []
for y in range(60):
    for x in range(80):
        p = [0, 0, 0, 0]
        if 10 <= x < 70 and 10 <= y < 50:
            p = [220, 72, 72, 255] if x < 40 else [55, 125, 225, 255]
        if (20 <= x < 60 and y == 25) or (x == 40 and 26 <= y < 43) or (x == 4 and y == 4):
            p = [0, 0, 0, 255]
        if 20 <= x < 28 and 35 <= y < 43:
            p = [0, 0, 0, 0]
        pixels.extend(p)
png('examples/color-card.png', 80, 60, pixels)
pixels = []
for y in range(100):
    for x in range(160):
        v = 150 + x // 2
        if (20 <= x < 140 and y == 40) or (x == 80 and 41 <= y < 80):
            v -= 35
        if (x, y) in {(50, 10), (51, 10)}:
            v = 0
        pixels.extend([v, v, v, 255])
png('examples/clean-scan.png', 160, 100, pixels)
