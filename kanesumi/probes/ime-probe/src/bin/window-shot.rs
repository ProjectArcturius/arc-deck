// Saiku Deck · 窗口截图
//
// 为什么需要它：输入链路的验证只到「App 收到事件」为止，而「事件是否真的画到
// 屏幕上」是另一段路径（Scene 构建 → 光栅化）。用 PrintWindow 抓窗口像素，
// 就能把渲染结果落成文件供直接检视 —— 无需人眼实时盯窗口，且可留档比对。
//
// 用法：window-shot <输出.bmp> [窗口类名]

use std::mem::zeroed;

const DEFAULT_CLASS: &[u16] = &[
    b'A' as u16, b'r' as u16, b'c' as u16, b'D' as u16, b'e' as u16, b'c' as u16, b'k' as u16,
    b'S' as u16, b'h' as u16, b'e' as u16, b'l' as u16, b'l' as u16, 0,
];

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct BitmapInfoHeader {
    size: u32,
    width: i32,
    height: i32,
    planes: u16,
    bit_count: u16,
    compression: u32,
    size_image: u32,
    x_pels: i32,
    y_pels: i32,
    clr_used: u32,
    clr_important: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct RgbQuad {
    blue: u8,
    green: u8,
    red: u8,
    reserved: u8,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct BitmapInfo {
    header: BitmapInfoHeader,
    colors: [RgbQuad; 1],
}

const BI_RGB: u32 = 0;
const PW_RENDERFULLCONTENT: u32 = 0x0000_0002;
const DIB_RGB_COLORS: u32 = 0;

#[link(name = "user32")]
unsafe extern "system" {
    fn FindWindowW(class: *const u16, title: *const u16) -> isize;
    fn GetClientRect(hwnd: isize, rect: *mut RectI) -> i32;
    fn GetDC(hwnd: isize) -> isize;
    fn ReleaseDC(hwnd: isize, dc: isize) -> i32;
    fn PrintWindow(hwnd: isize, dc: isize, flags: u32) -> i32;
}

#[link(name = "gdi32")]
unsafe extern "system" {
    fn CreateCompatibleDC(dc: isize) -> isize;
    fn CreateCompatibleBitmap(dc: isize, w: i32, h: i32) -> isize;
    fn SelectObject(dc: isize, obj: isize) -> isize;
    fn GetDIBits(
        dc: isize,
        bmp: isize,
        start: u32,
        lines: u32,
        bits: *mut u8,
        info: *mut BitmapInfo,
        usage: u32,
    ) -> i32;
    fn DeleteObject(obj: isize) -> i32;
    fn DeleteDC(dc: isize) -> i32;
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct RectI {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| "window-shot.bmp".to_string());

    // SAFETY: 标准 Win32 抓图序列；所有句柄使用前判空，末尾成对释放。
    unsafe {
        let hwnd = FindWindowW(DEFAULT_CLASS.as_ptr(), std::ptr::null());
        if hwnd == 0 {
            eprintln!("未找到 Saiku Deck 窗口");
            std::process::exit(1);
        }

        let mut rc: RectI = zeroed();
        if GetClientRect(hwnd, &mut rc) == 0 {
            eprintln!("GetClientRect 失败");
            std::process::exit(1);
        }
        let w = rc.right - rc.left;
        let h = rc.bottom - rc.top;
        if w <= 0 || h <= 0 {
            eprintln!("客户区尺寸异常: {w}×{h}");
            std::process::exit(1);
        }
        println!("客户区 {w}×{h}");

        let screen_dc = GetDC(hwnd);
        if screen_dc == 0 {
            eprintln!("GetDC 失败");
            std::process::exit(1);
        }
        let mem_dc = CreateCompatibleDC(screen_dc);
        let bmp = CreateCompatibleBitmap(screen_dc, w, h);
        let old = SelectObject(mem_dc, bmp);

        // PW_RENDERFULLCONTENT：抓取完整的客户区内容，而非仅可见部分。
        let ok = PrintWindow(hwnd, mem_dc, PW_RENDERFULLCONTENT);
        if ok == 0 {
            eprintln!("PrintWindow 返回 0（部分窗口不支持，但内容可能仍有效）");
        }

        // 取像素：BMP 自底向上，行按 4 字节对齐。
        let row_stride = ((w * 3 + 3) / 4) * 4;
        let mut pixels = vec![0u8; (row_stride * h) as usize];
        let mut info = BitmapInfo {
            header: BitmapInfoHeader {
                size: std::mem::size_of::<BitmapInfoHeader>() as u32,
                width: w,
                height: -h, // 负值 = 自顶向下，省一次翻转
                planes: 1,
                bit_count: 24,
                compression: BI_RGB,
                size_image: 0,
                x_pels: 0,
                y_pels: 0,
                clr_used: 0,
                clr_important: 0,
            },
            colors: [RgbQuad::default()],
        };
        let lines = GetDIBits(
            mem_dc,
            bmp,
            0,
            h as u32,
            pixels.as_mut_ptr(),
            &mut info,
            DIB_RGB_COLORS,
        );
        if lines == 0 {
            eprintln!("GetDIBits 失败");
        }

        // 像素是 BGRA（自顶向下），PNG 要 RGB —— 顺手转掉，PNG 里就不必再处理。
        let mut rgb = Vec::with_capacity((w * h * 3) as usize);
        for y in 0..h {
            let row = (y * row_stride) as usize;
            for x in 0..w {
                let px = row + (x * 3) as usize;
                // BMP 24bpp 的字节序是 B,G,R。
                rgb.push(pixels[px + 2]);
                rgb.push(pixels[px + 1]);
                rgb.push(pixels[px]);
            }
        }

        match write_png(&out, w as u32, h as u32, &rgb) {
            Ok(()) => println!("已写出 {out}（{w}×{h} RGB PNG）"),
            Err(e) => {
                eprintln!("写 PNG 失败: {e}");
                std::process::exit(1);
            }
        }

        // 释放。
        SelectObject(mem_dc, old);
        DeleteObject(bmp);
        DeleteDC(mem_dc);
        ReleaseDC(hwnd, screen_dc);
    }
}

// ── 最小 PNG 编码器 ────────────────────────────────────────────────
//
// 自写而不引图像库：本工具的职责只是「抓像素并落成可检视的文件」。
// TRUE_COLOR_8 + filter 0（None）+ zlib 容器，足够且无歧义。

fn crc32(data: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for (n, entry) in table.iter_mut().enumerate() {
        let mut c = n as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
        }
        *entry = c;
    }
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc = table[((crc ^ b as u32) & 0xFF) as usize] ^ (crc >> 8);
    }
    crc ^ 0xFFFF_FFFF
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], payload: &[u8]) {
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(payload);
    let mut crc_input = Vec::with_capacity(4 + payload.len());
    crc_input.extend_from_slice(kind);
    crc_input.extend_from_slice(payload);
    out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
}

fn write_png(path: &str, w: u32, h: u32, rgb: &[u8]) -> std::io::Result<()> {
    use flate2::write::ZlibEncoder;
    use flate2::Compression;
    use std::io::Write;

    // 每行前置一个 filter 字节（0 = None）。
    let mut raw = Vec::with_capacity((h * (1 + w * 3)) as usize);
    for y in 0..h {
        raw.push(0u8);
        let start = (y * w * 3) as usize;
        raw.extend_from_slice(&rgb[start..start + (w * 3) as usize]);
    }

    let mut z = ZlibEncoder::new(Vec::new(), Compression::default());
    z.write_all(&raw)?;
    let compressed = z.finish()?;

    let mut out = Vec::new();
    out.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);

    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.push(8); // bit depth
    ihdr.push(2); // color type: TRUE_COLOR
    ihdr.push(0); // compression
    ihdr.push(0); // filter
    ihdr.push(0); // interlace
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &compressed);
    chunk(&mut out, b"IEND", &[]);

    std::fs::write(path, out)
}
