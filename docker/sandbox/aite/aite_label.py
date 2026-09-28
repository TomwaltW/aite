"""Aite 生成文件的 AIGC 隐式标识器（owner: CC12；调用方 DD5 / EE12）。

镜像里的位置：/opt/aite/aite_label.py。只用标准库 + 镜像里已有的 pillow / openpyxl /
python-docx；pypdf 懒 import（D12 批了才有，缺包时 PDF 走「跳过」）。

用法（CLI）：

    python /opt/aite/aite_label.py [--producer 名] [--produce-id ID] [--propagator 名]
                                   [--propagate-id ID] [--watermark] <路径>...

每个路径往 stdout 打一行 JSON：{"path", "format", "status", "reason"}，
status ∈ labeled | skipped | error。

退出码：
    0  全部已标
    3  有跳过、无错误（不支持的扩展名 / 镜像里没有 pypdf）
    1  有文件出错
    2  用法错（argparse 默认）

**调用方约定：0 与 3 都算成功，只有 1 / 2 算失败**——不能拿 returncode != 0 判失败，
DD5 在 fetch 前批量调，一个 .csv 被跳过不该让任务失败。

用法（import）：

    import sys; sys.path.insert(0, "/opt/aite"); import aite_label
    aite_label.label_file(path, fields=None, watermark=False) -> dict   # 同 CLI 的一行
    aite_label.read_label(path) -> dict | None                          # 读回 5 个字段

写法：同目录临时文件 + os.replace，保留原权限位；同一文件标两次是替换、不重复。
标完文件的 size / mtime 会变，所以它会出现在那次 Exec 的 files_out 里。
"""

from __future__ import annotations

import argparse
import io
import json
import os
import re
import stat
import struct
import sys
import tempfile
import uuid
import xml.etree.ElementTree as ET
import zipfile

# ---------------------------------------------------------------- 载荷布局
# 布局来自第三方对 GB 45438-2025 的解读，FF6 买标准原文后核对（总计划 §10）。
# 5 个键名、Label 取值、XMP 命名空间都只在这一处定义。
AIGC_KEYS = ("Label", "ContentProducer", "ProduceID", "ContentPropagator", "PropagateID")
LABEL_VALUE = "1"
DEFAULT_PRODUCER = "Aite"
XMP_NS = "http://www.tc260.org.cn/ns/AIGC/1.0/"
XMP_PREFIX = "TC260"
XMP_PROP = "AIGC"
PNG_KEYWORD = "AIGC"
OOXML_PROP_NAME = "AIGC"
MD_KEY = "AIGC"
PDF_KEY = "/AIGC"

EXIT_OK = 0
EXIT_ERROR = 1
EXIT_USAGE = 2
EXIT_SKIPPED = 3

PDF_SKIP_REASON = "镜像里没有 pypdf（D12 未批），PDF 不打隐式标识"
WATERMARK_TEXT = "AI生成"
# `fc-list | grep -i wqy` 在镜像里实测的路径（fonts-wqy-microhei）。
WATERMARK_FONT = "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc"

_FORMATS = {
    ".png": "png",
    ".jpg": "jpeg",
    ".jpeg": "jpeg",
    ".docx": "docx",
    ".xlsx": "xlsx",
    ".pptx": "pptx",
    ".md": "markdown",
    ".markdown": "markdown",
    ".pdf": "pdf",
}


def make_fields(producer=None, produce_id=None, propagator=None, propagate_id=None) -> dict:
    """默认载荷：Label="1"、ContentProducer="Aite"、ProduceID=uuid4().hex、传播方两项为空。"""
    return {
        "Label": LABEL_VALUE,
        "ContentProducer": DEFAULT_PRODUCER if producer is None else str(producer),
        "ProduceID": uuid.uuid4().hex if produce_id is None else str(produce_id),
        "ContentPropagator": "" if propagator is None else str(propagator),
        "PropagateID": "" if propagate_id is None else str(propagate_id),
    }


def _normalize(fields) -> dict:
    base = make_fields()
    if fields:
        unknown = set(fields) - set(AIGC_KEYS)
        if unknown:
            raise ValueError(f"未知的 AIGC 字段：{sorted(unknown)}")
        base.update({k: str(v) for k, v in fields.items()})
    return {k: base[k] for k in AIGC_KEYS}


def _payload(fields, ascii_only=False) -> str:
    return json.dumps(fields, ensure_ascii=ascii_only, separators=(",", ":"))


def _parse_payload(text):
    try:
        obj = json.loads(text)
    except (TypeError, ValueError):
        return None
    return obj if isinstance(obj, dict) else None


def detect_format(path) -> str:
    ext = os.path.splitext(str(path))[1].lower()
    return _FORMATS.get(ext, ext.lstrip(".") or "unknown")


def _atomic_write(path, data: bytes) -> None:
    mode = stat.S_IMODE(os.stat(path).st_mode)
    d = os.path.dirname(os.path.abspath(path))
    fd, tmp = tempfile.mkstemp(dir=d, prefix=".aite_label.", suffix=".tmp")
    try:
        with os.fdopen(fd, "wb") as f:
            f.write(data)
        os.chmod(tmp, mode)
        os.replace(tmp, path)
    except BaseException:
        try:
            os.unlink(tmp)
        except FileNotFoundError:
            pass
        raise


def _read(path) -> bytes:
    with open(path, "rb") as f:
        return f.read()


# ---------------------------------------------------------------- 可见水印（可选，默认关）


def _watermark(im):
    from PIL import ImageDraw, ImageFont

    if not os.path.exists(WATERMARK_FONT):
        raise RuntimeError(f"水印字体不存在：{WATERMARK_FONT}")
    if im.mode not in ("RGB", "RGBA"):
        im = im.convert("RGBA" if ("A" in im.mode or "transparency" in im.info) else "RGB")
    else:
        im = im.copy()
    w, h = im.size
    size = max(12, min(w, h) // 12)
    font = ImageFont.truetype(WATERMARK_FONT, size)
    draw = ImageDraw.Draw(im)
    left, top, right, bottom = draw.textbbox((0, 0), WATERMARK_TEXT, font=font, stroke_width=1)
    margin = max(4, size // 3)
    xy = (w - (right - left) - margin - left, h - (bottom - top) - margin - top)
    fill = (255, 255, 255, 255) if im.mode == "RGBA" else (255, 255, 255)
    stroke = (0, 0, 0, 255) if im.mode == "RGBA" else (0, 0, 0)
    draw.text(xy, WATERMARK_TEXT, font=font, fill=fill, stroke_width=1, stroke_fill=stroke)
    return im


# ---------------------------------------------------------------- PNG：iTXt 块


def _png_chunks(data: bytes):
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        raise ValueError("不是 PNG 文件")
    pos = 8
    while pos + 8 <= len(data):
        (length,) = struct.unpack(">I", data[pos : pos + 4])
        ctype = data[pos + 4 : pos + 8]
        yield ctype, data[pos + 8 : pos + 8 + length]
        pos += 12 + length
        if ctype == b"IEND":
            break


def _png_text_value(ctype: bytes, body: bytes):
    """返回 (keyword, text)；不是文字块返回 None。"""
    import zlib

    if ctype == b"tEXt":
        k, _, v = body.partition(b"\x00")
        return k.decode("latin-1"), v.decode("latin-1")
    if ctype == b"iTXt":
        k, _, rest = body.partition(b"\x00")
        flag, _method = rest[0], rest[1]
        _lang, _, rest = rest[2:].partition(b"\x00")
        _tkey, _, text = rest.partition(b"\x00")
        if flag:
            text = zlib.decompress(text)
        return k.decode("latin-1"), text.decode("utf-8")
    if ctype == b"zTXt":
        k, _, rest = body.partition(b"\x00")
        return k.decode("latin-1"), zlib.decompress(rest[1:]).decode("latin-1")
    return None


def _label_png(path, fields, watermark):
    from PIL import Image, PngImagePlugin

    with Image.open(path) as src:
        src.load()
        if src.format != "PNG":
            raise ValueError(f"扩展名是 .png，内容是 {src.format}")
        info = PngImagePlugin.PngInfo()
        for k, v in getattr(src, "text", {}).items():
            if k != PNG_KEYWORD:
                info.add_text(k, v)
        info.add_itxt(PNG_KEYWORD, _payload(fields))
        kwargs = {"pnginfo": info}
        for key in ("icc_profile", "dpi", "transparency", "gamma"):
            if key in src.info:
                kwargs[key] = src.info[key]
        im = _watermark(src) if watermark else src
        if im is not src:
            kwargs.pop("transparency", None)
        buf = io.BytesIO()
        im.save(buf, format="PNG", **kwargs)
    _atomic_write(path, buf.getvalue())


def _read_png(path):
    for ctype, body in _png_chunks(_read(path)):
        kv = _png_text_value(ctype, body)
        if kv and kv[0] == PNG_KEYWORD:
            return _parse_payload(kv[1])
    return None


# ---------------------------------------------------------------- JPEG：XMP(APP1) + EXIF UserComment
# 按字节换 APP1 段，扫描数据（SOS 之后）原样保留 → 不重压缩像素。
# 只有 --watermark 时才会重编码（quality=95）。

_XMP_HEADER = b"http://ns.adobe.com/xap/1.0/\x00"
_EXIF_HEADER = b"Exif\x00\x00"
_USER_COMMENT_ASCII = b"ASCII\x00\x00\x00"
_EXIF_IFD = 0x8769
_INTEROP_IFD = 0xA005
_USER_COMMENT = 0x9286
_RDF_NS = "http://www.w3.org/1999/02/22-rdf-syntax-ns#"
_X_NS = "adobe:ns:meta/"


def _jpeg_split(data: bytes):
    """返回 (段列表[(marker, payload)], 从 SOS 起的剩余字节)。"""
    if data[:2] != b"\xff\xd8":
        raise ValueError("不是 JPEG 文件")
    pos, segs = 2, []
    while pos < len(data):
        if data[pos] != 0xFF:
            raise ValueError(f"JPEG 段头损坏（偏移 {pos}）")
        while data[pos + 1] == 0xFF:
            pos += 1
        marker = data[pos + 1]
        if marker in (0xDA, 0xD9):
            return segs, data[pos:]
        if 0xD0 <= marker <= 0xD7 or marker == 0x01:
            segs.append((marker, None))
            pos += 2
            continue
        (length,) = struct.unpack(">H", data[pos + 2 : pos + 4])
        segs.append((marker, data[pos + 4 : pos + 2 + length]))
        pos += 2 + length
    raise ValueError("JPEG 没有扫描数据")


def _jpeg_join(segs, rest) -> bytes:
    out = bytearray(b"\xff\xd8")
    for marker, payload in segs:
        out += bytes((0xFF, marker))
        if payload is not None:
            if len(payload) + 2 > 0xFFFF:
                raise ValueError("APP 段超过 64KB")
            out += struct.pack(">H", len(payload) + 2) + payload
    return bytes(out + rest)


def _xmp_packet(existing, payload: str) -> bytes:
    ET.register_namespace(XMP_PREFIX, XMP_NS)
    ET.register_namespace("rdf", _RDF_NS)
    ET.register_namespace("x", _X_NS)
    attr = f"{{{XMP_NS}}}{XMP_PROP}"
    if existing:
        root = ET.fromstring(existing.decode("utf-8").strip().strip("\x00"))
        rdf = root if root.tag == f"{{{_RDF_NS}}}RDF" else root.find(f".//{{{_RDF_NS}}}RDF")
        if rdf is None:
            raise ValueError("已有 XMP 里找不到 rdf:RDF")
    else:
        root = ET.Element(f"{{{_X_NS}}}xmpmeta")
        rdf = ET.SubElement(root, f"{{{_RDF_NS}}}RDF")
    desc = None
    for d in rdf.findall(f"{{{_RDF_NS}}}Description"):
        if attr in d.attrib:
            desc = d
            break
    if desc is None:
        desc = ET.SubElement(rdf, f"{{{_RDF_NS}}}Description", {f"{{{_RDF_NS}}}about": ""})
    desc.set(attr, payload)
    body = ET.tostring(root, encoding="unicode")
    packet = '<?xpacket begin="﻿" id="W5M0MpCehiHzreSzNTczkc9d"?>' + body + '<?xpacket end="w"?>'
    return packet.encode("utf-8")


def _xmp_value(packet: bytes):
    try:
        root = ET.fromstring(packet.decode("utf-8").strip().strip("\x00"))
    except (ET.ParseError, UnicodeDecodeError):
        return None
    attr = f"{{{XMP_NS}}}{XMP_PROP}"
    for el in root.iter():
        if attr in el.attrib:
            return el.attrib[attr]
    return None


def _exif_payload(existing, fields) -> bytes:
    from PIL import Image

    exif = Image.Exif()
    if existing:
        exif.load(existing)
    # pillow 11.0.0 实测：只改 get_ifd() 返回的缓存，tag 不在顶层时 tobytes() 会把整个 Exif IFD 丢掉；
    # 要把子 IFD 当 dict 显式塞回顶层（Interop 子 IFD 同理）。
    sub = dict(exif.get_ifd(_EXIF_IFD))
    if _INTEROP_IFD in sub and not isinstance(sub[_INTEROP_IFD], dict):
        sub[_INTEROP_IFD] = dict(exif.get_ifd(_INTEROP_IFD))
    sub[_USER_COMMENT] = _USER_COMMENT_ASCII + _payload(fields, ascii_only=True).encode("ascii")
    exif[_EXIF_IFD] = sub
    return exif.tobytes()


def _label_jpeg(path, fields, watermark):
    data = _read(path)
    if watermark:
        from PIL import Image

        with Image.open(io.BytesIO(data)) as src:
            src.load()
            im = _watermark(src).convert("RGB")
            buf = io.BytesIO()
            kwargs = {"quality": 95}
            if "icc_profile" in src.info:
                kwargs["icc_profile"] = src.info["icc_profile"]
            if "exif" in src.info:
                kwargs["exif"] = src.info["exif"]
            im.save(buf, format="JPEG", **kwargs)
        data = buf.getvalue()
    segs, rest = _jpeg_split(data)
    old_exif = old_xmp = None
    kept = []
    for marker, payload in segs:
        if marker == 0xE1 and payload is not None and payload.startswith(_EXIF_HEADER):
            old_exif = old_exif or payload
            continue
        if marker == 0xE1 and payload is not None and payload.startswith(_XMP_HEADER):
            old_xmp = old_xmp or payload[len(_XMP_HEADER) :]
            continue
        kept.append((marker, payload))
    new = [
        (0xE1, _exif_payload(old_exif, fields)),
        (0xE1, _XMP_HEADER + _xmp_packet(old_xmp, _payload(fields))),
    ]
    # JFIF(APP0) 若有，留在最前面。
    head = [s for s in kept[:1] if s[0] == 0xE0]
    tail = kept[len(head) :]
    _atomic_write(path, _jpeg_join(head + new + tail, rest))


def _read_jpeg(path):
    segs, _ = _jpeg_split(_read(path))
    for marker, payload in segs:
        if marker == 0xE1 and payload is not None and payload.startswith(_XMP_HEADER):
            value = _xmp_value(payload[len(_XMP_HEADER) :])
            if value is not None:
                return _parse_payload(value)
    for marker, payload in segs:
        if marker == 0xE1 and payload is not None and payload.startswith(_EXIF_HEADER):
            from PIL import Image

            exif = Image.Exif()
            exif.load(payload)
            raw = exif.get_ifd(_EXIF_IFD).get(_USER_COMMENT)
            if isinstance(raw, bytes) and raw.startswith(_USER_COMMENT_ASCII):
                return _parse_payload(raw[len(_USER_COMMENT_ASCII) :].decode("ascii", "replace"))
    return None


# ---------------------------------------------------------------- OOXML：docProps/custom.xml

_CT_NS = "http://schemas.openxmlformats.org/package/2006/content-types"
_REL_NS = "http://schemas.openxmlformats.org/package/2006/relationships"
_CUSTOM_NS = "http://schemas.openxmlformats.org/officeDocument/2006/custom-properties"
_VT_NS = "http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes"
_CUSTOM_PART = "docProps/custom.xml"
_CUSTOM_CT = "application/vnd.openxmlformats-officedocument.custom-properties+xml"
_CUSTOM_REL = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/custom-properties"
_CUSTOM_FMTID = "{D5CDD505-2E9C-101B-9397-08002B2CF9AE}"


def _xml_bytes(root, default_ns) -> bytes:
    # 不用 tostring(default_namespace=…)：它遇到不带命名空间的属性（PartName、Id…）直接报错。
    # 把默认命名空间登记成空前缀（同前缀的旧登记会被替换），每次序列化前重登一次。
    ET.register_namespace("vt", _VT_NS)
    ET.register_namespace("", default_ns)
    return ET.tostring(root, encoding="UTF-8", xml_declaration=True)


def _custom_xml(existing, payload: str) -> bytes:
    if existing:
        root = ET.fromstring(existing)
    else:
        root = ET.Element(f"{{{_CUSTOM_NS}}}Properties")
    props = root.findall(f"{{{_CUSTOM_NS}}}property")
    target = next((p for p in props if p.get("name") == OOXML_PROP_NAME), None)
    if target is None:
        pids = [int(p.get("pid")) for p in props if (p.get("pid") or "").isdigit()]
        target = ET.SubElement(
            root,
            f"{{{_CUSTOM_NS}}}property",
            {"fmtid": _CUSTOM_FMTID, "pid": str(max(pids + [1]) + 1), "name": OOXML_PROP_NAME},
        )
    for child in list(target):
        target.remove(child)
    ET.SubElement(target, f"{{{_VT_NS}}}lpwstr").text = payload
    return _xml_bytes(root, _CUSTOM_NS)


def _content_types(data: bytes) -> bytes:
    root = ET.fromstring(data)
    for o in root.findall(f"{{{_CT_NS}}}Override"):
        if o.get("PartName") == "/" + _CUSTOM_PART:
            if o.get("ContentType") == _CUSTOM_CT:
                return data
            o.set("ContentType", _CUSTOM_CT)
            return _xml_bytes(root, _CT_NS)
    ET.SubElement(root, f"{{{_CT_NS}}}Override", {"PartName": "/" + _CUSTOM_PART, "ContentType": _CUSTOM_CT})
    return _xml_bytes(root, _CT_NS)


def _package_rels(data) -> bytes:
    if data:
        root = ET.fromstring(data)
    else:
        root = ET.Element(f"{{{_REL_NS}}}Relationships")
    rels = root.findall(f"{{{_REL_NS}}}Relationship")
    for r in rels:
        if r.get("Type") == _CUSTOM_REL:
            return data
    ids = {r.get("Id") for r in rels}
    n = 1
    while f"rId{n}" in ids:
        n += 1
    ET.SubElement(root, f"{{{_REL_NS}}}Relationship", {"Id": f"rId{n}", "Type": _CUSTOM_REL, "Target": _CUSTOM_PART})
    return _xml_bytes(root, _REL_NS)


def _label_ooxml(path, fields, watermark):
    with zipfile.ZipFile(path) as zin:
        infos = zin.infolist()
        parts = {i.filename: zin.read(i.filename) for i in infos}
    if "[Content_Types].xml" not in parts:
        raise ValueError("不是 OOXML 包：缺 [Content_Types].xml")
    parts[_CUSTOM_PART] = _custom_xml(parts.get(_CUSTOM_PART), _payload(fields))
    parts["[Content_Types].xml"] = _content_types(parts["[Content_Types].xml"])
    parts["_rels/.rels"] = _package_rels(parts.get("_rels/.rels"))
    order = [i.filename for i in infos]
    for extra in ("_rels/.rels", _CUSTOM_PART):
        if extra not in order:
            order.append(extra)
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w", zipfile.ZIP_DEFLATED) as zout:
        for name in order:
            zout.writestr(name, parts[name])
    _atomic_write(path, buf.getvalue())


def _read_ooxml(path):
    with zipfile.ZipFile(path) as z:
        if _CUSTOM_PART not in z.namelist():
            return None
        root = ET.fromstring(z.read(_CUSTOM_PART))
    for p in root.findall(f"{{{_CUSTOM_NS}}}property"):
        if p.get("name") == OOXML_PROP_NAME:
            v = p.find(f"{{{_VT_NS}}}lpwstr")
            return _parse_payload(v.text if v is not None else None)
    return None


# ---------------------------------------------------------------- Markdown：YAML front-matter

_FM_OPEN = re.compile(r"\A(﻿?)---[ \t]*\r?\n")
_FM_CLOSE = re.compile(r"^(?:---|\.\.\.)[ \t]*\r?$", re.M)
_MD_LINE = re.compile(rf"^{MD_KEY}[ \t]*:")


def _split_front_matter(text):
    """返回 (bom, front-matter 行列表, 正文)；没有 front-matter 返回 (bom, None, 正文)。"""
    m = _FM_OPEN.match(text)
    if not m:
        bom = "﻿" if text.startswith("﻿") else ""
        return bom, None, text[len(bom) :]
    close = _FM_CLOSE.search(text, m.end())
    if not close:
        bom = m.group(1)
        return bom, None, text[len(bom) :]
    block = text[m.end() : close.start()]
    end = close.end()
    if text[end : end + 2] == "\r\n":
        end += 2
    elif text[end : end + 1] == "\n":
        end += 1
    return m.group(1), block.splitlines(), text[end:]


def _yaml_single_quoted(s: str) -> str:
    return "'" + s.replace("'", "''") + "'"


def _label_markdown(path, fields, watermark):
    text = _read(path).decode("utf-8")
    bom, lines, body = _split_front_matter(text)
    entry = f"{MD_KEY}: {_yaml_single_quoted(_payload(fields))}"
    if lines is None:
        out = f"{bom}---\n{entry}\n---\n{body}"
    else:
        kept, skipping = [], False
        for line in lines:
            if _MD_LINE.match(line):
                skipping = True
                continue
            if skipping and line[:1] in (" ", "\t"):
                continue
            skipping = False
            kept.append(line)
        kept.append(entry)
        out = bom + "---\n" + "\n".join(kept) + "\n---\n" + body
    _atomic_write(path, out.encode("utf-8"))


def _read_markdown(path):
    _bom, lines, _body = _split_front_matter(_read(path).decode("utf-8"))
    for line in lines or []:
        if _MD_LINE.match(line):
            value = line.split(":", 1)[1].strip()
            if len(value) >= 2 and value[0] == value[-1] == "'":
                value = value[1:-1].replace("''", "'")
            return _parse_payload(value)
    return None


# ---------------------------------------------------------------- PDF：文档信息字典 /AIGC（pypdf 懒 import）


class _Skip(Exception):
    pass


def _pypdf():
    try:
        import pypdf
    except ImportError:
        raise _Skip(PDF_SKIP_REASON) from None
    return pypdf


def _label_pdf(path, fields, watermark):
    pypdf = _pypdf()
    writer = pypdf.PdfWriter(clone_from=path)
    writer.add_metadata({PDF_KEY: _payload(fields)})
    buf = io.BytesIO()
    writer.write(buf)
    _atomic_write(path, buf.getvalue())


def _read_pdf(path):
    try:
        pypdf = _pypdf()
    except _Skip:
        return None
    meta = pypdf.PdfReader(path).metadata or {}
    value = meta.get(PDF_KEY)
    return _parse_payload(str(value)) if value is not None else None


# ---------------------------------------------------------------- 入口

_WRITERS = {
    "png": _label_png,
    "jpeg": _label_jpeg,
    "docx": _label_ooxml,
    "xlsx": _label_ooxml,
    "pptx": _label_ooxml,
    "markdown": _label_markdown,
    "pdf": _label_pdf,
}
_READERS = {
    "png": _read_png,
    "jpeg": _read_jpeg,
    "docx": _read_ooxml,
    "xlsx": _read_ooxml,
    "pptx": _read_ooxml,
    "markdown": _read_markdown,
    "pdf": _read_pdf,
}


def label_file(path, fields=None, watermark=False) -> dict:
    """给一个文件打隐式标识；返回与 CLI 同形的 dict（不抛异常，错误落在 status=error）。

    watermark 只对 PNG / JPEG 生效（右下角画「AI生成」），默认关。
    """
    path = os.fspath(path)
    fmt = detect_format(path)
    result = {"path": path, "format": fmt, "status": "labeled", "reason": ""}
    try:
        if not os.path.isfile(path):
            raise FileNotFoundError(f"文件不存在：{path}")
        writer = _WRITERS.get(fmt)
        if writer is None:
            result.update(status="skipped", reason=f"不支持的类型（{fmt}），不打隐式标识")
            return result
        writer(path, _normalize(fields), watermark and fmt in ("png", "jpeg"))
    except _Skip as e:
        result.update(status="skipped", reason=str(e))
    except Exception as e:  # noqa: BLE001 —— 一个文件出错不影响同批其它文件
        result.update(status="error", reason=f"{type(e).__name__}: {e}")
    return result


def read_label(path):
    """读回 5 个字段的 dict；没标过 / 不支持 / 读不出返回 None。"""
    reader = _READERS.get(detect_format(path))
    if reader is None:
        return None
    try:
        return reader(os.fspath(path))
    except Exception:  # noqa: BLE001
        return None


def exit_code_for(results) -> int:
    statuses = {r["status"] for r in results}
    if "error" in statuses:
        return EXIT_ERROR
    if "skipped" in statuses:
        return EXIT_SKIPPED
    return EXIT_OK


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(prog="aite_label.py", description="给生成文件打 AIGC 隐式标识")
    ap.add_argument("--producer", default=None, help=f"ContentProducer（默认 {DEFAULT_PRODUCER}）")
    ap.add_argument("--produce-id", default=None, help="ProduceID（默认本次调用生成一个 uuid4 hex，各文件共用）")
    ap.add_argument("--propagator", default=None, help="ContentPropagator（默认空）")
    ap.add_argument("--propagate-id", default=None, help="PropagateID（默认空）")
    ap.add_argument("--watermark", action="store_true", help="PNG / JPEG 右下角加可见的「AI生成」（默认关）")
    ap.add_argument("paths", nargs="+", metavar="路径")
    args = ap.parse_args(argv)
    fields = make_fields(args.producer, args.produce_id, args.propagator, args.propagate_id)
    results = []
    for p in args.paths:
        r = label_file(p, fields, watermark=args.watermark)
        results.append(r)
        print(json.dumps(r, ensure_ascii=False), flush=True)
    return exit_code_for(results)


if __name__ == "__main__":
    sys.exit(main())
