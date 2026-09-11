//! 场景用的小样本数据（对应旧 `aite/testing/samples.py`）。不引第三方依赖，全部现造。
//!
//! 与 Python 版的唯一差异：`zlib.compress` 在这里换成**手写的 stored（非压缩）deflate 块**
//! —— workspace 没钉 flate2 / crc32fast，加依赖要停下报告（spec §7.1 第 3 条）。
//! 产物仍是一张合法 PNG（魔数 + IHDR + IDAT(zlib 容器) + IEND，每块带 CRC32），
//! 只是 IDAT 段的字节比 Python 版长几字节。04_csv_to_chart 断言的是前 8 字节魔数，不受影响。
use std::collections::BTreeMap;
use std::sync::LazyLock;

pub const PNG_MAGIC: [u8; 8] = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];

// ---- CRC32（IEEE，PNG 块校验用）------------------------------------------

static CRC_TABLE: LazyLock<[u32; 256]> = LazyLock::new(|| {
    let mut table = [0u32; 256];
    for (n, slot) in table.iter_mut().enumerate() {
        let mut c = n as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
        *slot = c;
    }
    table
});

pub fn crc32(data: &[u8]) -> u32 {
    let table = &*CRC_TABLE;
    let mut c = 0xFFFF_FFFFu32;
    for b in data {
        c = table[((c ^ u32::from(*b)) & 0xFF) as usize] ^ (c >> 8);
    }
    c ^ 0xFFFF_FFFF
}

// ---- adler32（zlib 尾校验）------------------------------------------------

pub fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for byte in data {
        a = (a + u32::from(*byte)) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

/// zlib 容器 + 全 stored 的 deflate 流。解码器一律认；只是不压缩。
fn zlib_stored(raw: &[u8]) -> Vec<u8> {
    let mut out = vec![0x78, 0x01]; // CMF/FLG：deflate 32K 窗口，(0x78<<8|0x01) % 31 == 0
    if raw.is_empty() {
        // 空数据也要有一个 final 的空 stored 块
        out.extend_from_slice(&[0x01, 0x00, 0x00, 0xFF, 0xFF]);
    } else {
        for (i, chunk) in raw.chunks(0xFFFF).enumerate() {
            let is_last = (i + 1) * 0xFFFF >= raw.len();
            out.push(if is_last { 0x01 } else { 0x00 }); // BFINAL + BTYPE=00，随即按字节对齐
            let len = chunk.len() as u16;
            out.extend_from_slice(&len.to_le_bytes());
            out.extend_from_slice(&(!len).to_le_bytes());
            out.extend_from_slice(chunk);
        }
    }
    out.extend_from_slice(&adler32(raw).to_be_bytes());
    out
}

fn chunk(tag: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut body = Vec::with_capacity(4 + data.len());
    body.extend_from_slice(tag);
    body.extend_from_slice(data);
    let mut out = Vec::with_capacity(12 + data.len());
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(&body);
    out.extend_from_slice(&crc32(&body).to_be_bytes());
    out
}

/// 造一张真能被解码的 PNG —— 04_csv_to_chart 要断言前 8 字节是 PNG 魔数。
pub fn png_bytes(width: u32, height: u32, rgb: (u8, u8, u8)) -> Vec<u8> {
    let mut row = Vec::with_capacity(1 + (width as usize) * 3);
    row.push(0u8); // 每行开头的 filter 字节（0 = None）
    for _ in 0..width {
        row.extend_from_slice(&[rgb.0, rgb.1, rgb.2]);
    }
    let raw = row.repeat(height as usize);

    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]); // bit_depth=8, color_type=2(RGB), 其余 0

    let mut out = Vec::new();
    out.extend_from_slice(&PNG_MAGIC);
    out.extend_from_slice(&chunk(b"IHDR", &ihdr));
    out.extend_from_slice(&chunk(b"IDAT", &zlib_stored(&raw)));
    out.extend_from_slice(&chunk(b"IEND", b""));
    out
}

pub static PNG_1X1: LazyLock<Vec<u8>> = LazyLock::new(|| png_bytes(1, 1, (255, 255, 255)));

pub const CSV_SAMPLE: &str = "month,amount\n2026-01,120\n2026-02,180\n2026-03,90\n";

/// 场景 yaml 的 `writes` / `files` 取值可以写成 `builtin:png`、`builtin:csv`，这里是兑现表。
pub static BUILTINS: LazyLock<BTreeMap<&'static str, Vec<u8>>> = LazyLock::new(|| {
    BTreeMap::from([
        ("png", PNG_1X1.clone()),
        ("csv", CSV_SAMPLE.as_bytes().to_vec()),
    ])
});
