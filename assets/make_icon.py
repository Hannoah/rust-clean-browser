"""Generates the app icon (no third-party deps).

    python assets/make_icon.py   ->  assets/icon.ico (Windows, 16..256 px), assets/icon.icns (macOS),
                                     assets/icon_64.rgba (window icon)
"""
import math
import os
import struct
import zlib

HERE = os.path.dirname(os.path.abspath(__file__))
SS = 4  # supersampling per axis


def lerp(a, b, t):
    return a + (b - a) * t


def coverage(nx, ny):
    """Returns RGBA (0..1 floats, premultiplied-free) for a normalized point in [0,1]^2."""
    # Rounded square (squircle-ish) background.
    r = 0.22
    dx = max(abs(nx - 0.5) - (0.5 - r), 0.0)
    dy = max(abs(ny - 0.5) - (0.5 - r), 0.0)
    if math.hypot(dx, dy) > r - 0.02:
        return None
    t = (nx + ny) / 2
    col = (lerp(0x1d, 0x06, t) / 255, lerp(0x4e, 0xb6, t) / 255, lerp(0xd8, 0xd4, t) / 255)

    white = (1.0, 1.0, 1.0)
    cx, cy = 0.47, 0.53
    d = math.hypot(nx - cx, ny - cy)
    R = 0.29
    # Globe outline.
    if abs(d - R) < 0.045:
        col = white
    elif d < R:
        # Meridian ellipse + equator.
        e = math.hypot((nx - cx) / 0.12, (ny - cy) / R)
        if abs(e - 1.0) < 0.16 or abs(ny - cy) < 0.026:
            col = white
    # Orange "Rust" spark at top-right with a white ring.
    sd = math.hypot(nx - 0.745, ny - 0.255)
    if sd < 0.095:
        col = (0xf9 / 255, 0x73 / 255, 0x16 / 255)
    elif sd < 0.135:
        col = white
    return col


def render(size):
    px = bytearray(size * size * 4)
    for y in range(size):
        for x in range(size):
            acc = [0.0, 0.0, 0.0, 0.0]
            for sy in range(SS):
                for sx in range(SS):
                    c = coverage((x + (sx + 0.5) / SS) / size, (y + (sy + 0.5) / SS) / size)
                    if c:
                        acc[0] += c[0]
                        acc[1] += c[1]
                        acc[2] += c[2]
                        acc[3] += 1
            n = SS * SS
            a = acc[3] / n
            i = (y * size + x) * 4
            if acc[3]:
                px[i:i + 4] = bytes((round(acc[0] / acc[3] * 255), round(acc[1] / acc[3] * 255),
                                     round(acc[2] / acc[3] * 255), round(a * 255)))
    return bytes(px)


def png(size, rgba):
    raw = b"".join(b"\x00" + rgba[y * size * 4:(y + 1) * size * 4] for y in range(size))

    def chunk(tag, data):
        return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b""))


def main():
    sizes = [16, 24, 32, 48, 64, 128, 256]
    images = {s: render(s) for s in sizes}
    pngs = [png(s, images[s]) for s in sizes]
    header = struct.pack("<HHH", 0, 1, len(sizes))
    offset = 6 + 16 * len(sizes)
    entries = b""
    for s, data in zip(sizes, pngs):
        entries += struct.pack("<BBBBHHII", s % 256, s % 256, 0, 0, 1, 32, len(data), offset)
        offset += len(data)
    with open(os.path.join(HERE, "icon.ico"), "wb") as f:
        f.write(header + entries + b"".join(pngs))
    with open(os.path.join(HERE, "icon_64.rgba"), "wb") as f:
        f.write(images[64])
    with open(os.path.join(HERE, "icon_256.png"), "wb") as f:
        f.write(pngs[-1])

    # macOS .icns: PNG-compressed entries (ic07=128, ic08=256, ic09=512 px).
    icns_entries = b""
    for tag, size in ((b"ic07", 128), (b"ic08", 256), (b"ic09", 512)):
        data = png(size, images[size] if size in images else render(size))
        icns_entries += tag + struct.pack(">I", len(data) + 8) + data
    with open(os.path.join(HERE, "icon.icns"), "wb") as f:
        f.write(b"icns" + struct.pack(">I", len(icns_entries) + 8) + icns_entries)


if __name__ == "__main__":
    main()
