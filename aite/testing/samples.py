"""场景用的小样本数据（不引第三方依赖，全部现造）。"""
from __future__ import annotations

import struct
import zlib

PNG_MAGIC = b"\x89PNG\r\n\x1a\n"


def png_bytes(width: int = 1, height: int = 1, rgb: tuple[int, int, int] = (255, 255, 255)) -> bytes:
    """造一张真能被解码的 PNG —— 04_csv_to_chart 要断言前 8 字节是 PNG 魔数。"""
    raw = b"".join(b"\x00" + bytes(rgb) * width for _ in range(height))

    def chunk(tag: bytes, data: bytes) -> bytes:
        body = tag + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF)

    ihdr = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
    return PNG_MAGIC + chunk(b"IHDR", ihdr) + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b"")


PNG_1X1 = png_bytes()

CSV_SAMPLE = "month,amount\n2026-01,120\n2026-02,180\n2026-03,90\n"

#: 场景 yaml 的 writes / files 取值可以写成 "builtin:png"、"builtin:csv"，这里是兑现表。
BUILTINS: dict[str, bytes] = {
    "png": PNG_1X1,
    "csv": CSV_SAMPLE.encode("utf-8"),
}
