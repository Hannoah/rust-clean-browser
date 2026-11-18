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


def mix(c1, c2, t):
    return tuple(lerp(a, b, t) for a, b in zip(c1, c2))


def hexc(h):
    return tuple(int(h[i:i + 2], 16) / 255 for i in (1, 3, 5))


BG_A, BG_B = hexc("#3b82f6"), hexc("#6d28d9")      # blue -> violet diagonal
PLANET_A, PLANET_B = hexc("#fdba74"), hexc("#ea580c")  # rust orange
WHITE = (1.0, 1.0, 1.0)
CX, CY, R, W = 0.5, 0.5, 0.235, 0.085               # orbit ring
GAP = (math.radians(18), math.radians(72))           # opening at the top right
PA = math.radians(45)                                 # planet sits in the opening
PX, PY, PR = CX + R * math.cos(PA), CY - R * math.sin(PA), 0.085


def in_ring(nx, ny):
    d = math.hypot(nx - CX, ny - CY)
    if abs(d - R) > W / 2:
        # Round caps at both ends of the opening.
        return any(math.hypot(nx - (CX + R * math.cos(a)), ny - (CY - R * math.sin(a))) <= W / 2 for a in GAP)
    ang = math.atan2(-(ny - CY), nx - CX) % (2 * math.pi)
    return not (GAP[0] < ang < GAP[1])


def coverage(nx, ny):
    """RGBA (0..1, straight alpha) of a normalized point in [0,1]^2, or None if transparent."""
    # Squircle (superellipse, n=5) with a small margin, like modern app icons.
    sx, sy = (nx - 0.5) / 0.46, (ny - 0.5) / 0.46
    if abs(sx) ** 5 + abs(sy) ** 5 > 1:
        return None
    col = mix(BG_A, BG_B, (nx + ny) / 2)
    # Soft highlight from the top-left.
    glow = max(0.0, 1 - math.hypot(nx - 0.25, ny - 0.18) / 0.55)
    col = mix(col, WHITE, 0.18 * glow * glow)
    # Faint drop shadow under the mark for depth.
    if in_ring(nx - 0.012, ny - 0.02) or math.hypot(nx - 0.012 - PX, ny - 0.02 - PY) <= PR:
        col = mix(col, (0.08, 0.04, 0.25), 0.28)
    if in_ring(nx, ny):
        col = WHITE
    pd = math.hypot(nx - PX, ny - PY)
    if pd <= PR:
        t = min(1.0, math.hypot(nx - (PX - PR * 0.4), ny - (PY - PR * 0.4)) / (PR * 1.6))
        col = mix(PLANET_A, PLANET_B, t)
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
