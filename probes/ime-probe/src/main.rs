// Arc Deck · IME 探针
//
// 问题（否决性）：在**自研 Win32 事件循环**里，中文 IME 能否正确组合并提交？
//
// 为什么不用 winit：egui 的 macOS IME 至今卡在 winit#4432，iced 的中文 panic 修复 PR
// 挂了半年。Kanesumi 的 Linux 外壳用 sctk + calloop 自研事件循环，不用 winit ——
// Windows 侧照此办理，直接写 Win32 消息泵，IME 完全归自己管。
//
// 本探针**不依赖任何 GUI 框架**，也不依赖 kanesumi-*（后者的 workspace 含 Linux-only
// 依赖，在 Windows 上无法整体解析）。它只回答一件事：IMM32 的 IME 消息通路是否畅通。
//
// 验证清单：
//   1. 窗口创建（无 wgpu / 无 wgpu 依赖）
//   2. WM_IME_* 消息到达
//   3. GCS_COMPSTR 取到组合串（拼音过程）
//   4. GCS_RESULTSTR 取到提交串（中文）
//   5. 通过 ImmSetCompositionWindow 把候选窗定位到光标

use std::ffi::c_void;
use std::mem::zeroed;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{GetLastError, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{GetStockObject, InvalidateRect, UpdateWindow, DEFAULT_GUI_FONT, HFONT};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::Ime::{
    ImmGetCompositionStringW, ImmGetContext, ImmReleaseContext, ImmSetCompositionWindow,
    CFS_POINT, COMPOSITIONFORM, GCS_COMPSTR, GCS_CURSORPOS, GCS_RESULTSTR, HIMC,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect, GetMessageW,
    GetWindowLongPtrW, LoadCursorW, PostQuitMessage, RegisterClassExW,
    SetWindowLongPtrW, SetWindowTextW, ShowWindow, TranslateMessage, CS_HREDRAW,
    CS_VREDRAW, CW_USEDEFAULT, GWLP_USERDATA, IDC_IBEAM, MSG, SW_SHOW, WM_CLOSE, WM_DESTROY,
    WM_IME_CHAR, WM_IME_COMPOSITION, WM_IME_ENDCOMPOSITION, WM_IME_SETCONTEXT,
    WM_IME_STARTCOMPOSITION, WNDCLASSEXW, WS_OVERLAPPEDWINDOW,
};

/// `RGB(r,g,b)` 是 Gdi 头文件里的 C 宏，windows-sys 不导出 —— 按其定义手写。
const fn rgb(r: u8, g: u8, b: u8) -> u32 {
    (r as u32) | ((g as u32) << 8) | ((b as u32) << 16)
}

/// 窗口类名（宽字符串，静态）。
const CLASS_NAME: &[u16] = &[
    b'A' as u16, b'r' as u16, b'c' as u16, b'D' as u16, b'e' as u16, b'c' as u16, b'k' as u16,
    b'I' as u16, b'm' as u16, b'e' as u16, b'P' as u16, b'r' as u16, b'o' as u16, b'b' as u16,
    b'e' as u16, 0,
];

/// 探针状态 —— 挂在窗口的 GWLP_USERDATA 上，避免全局可变状态。
struct Probe {
    /// 上次收到的组合串（拼音过程，例如 "zhong"）。
    composing: String,
    /// 已提交的文本（中文结果）。
    committed: String,
    /// 收到的事件计数，用来判断消息通路是否真的活了。
    ime_msg_count: u32,
    /// 组合串光标位置（GCS_CURSORPOS）。
    cursor_pos: i32,
}

impl Probe {
    fn new() -> Self {
        Self {
            composing: String::new(),
            committed: String::new(),
            ime_msg_count: 0,
            cursor_pos: 0,
        }
    }
}

/// 把 UTF-16 缓冲（可能带结尾 NUL）转成 String。
fn utf16_to_string(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

/// 取一笔 IME 组合串。返回 `None` 表示该标志本次无内容。
fn get_composition_string(himc: HIMC, flag: u32) -> Option<String> {
    // SAFETY: himc 由 ImmGetContext 取得且在调用方持有期间有效；传空缓冲 + 0 长度
    // 是 IMM32 约定的「问长度」用法。
    let byte_len = unsafe { ImmGetCompositionStringW(himc, flag, null_mut(), 0) };
    if byte_len <= 0 {
        return None;
    }
    let char_capacity = byte_len as usize / 2 + 1;
    let mut buf: Vec<u16> = vec![0; char_capacity];
    // SAFETY: buf 至少有 byte_len 字节容量；再次调用写入实际内容。
    let written = unsafe {
        ImmGetCompositionStringW(himc, flag, buf.as_mut_ptr() as *mut c_void, byte_len as u32)
    };
    if written <= 0 {
        return None;
    }
    buf.truncate(written as usize / 2);
    Some(utf16_to_string(&buf))
}

/// 把窗口标题更新为当前状态 —— 零渲染依赖的可视化手段。
fn refresh_title(hwnd: HWND, probe: &Probe) {
    let title = format!(
        "Arc Deck IME Probe | ime_msgs={} | composing=[{}] cur={} | committed=[{}]",
        probe.ime_msg_count, probe.composing, probe.cursor_pos, probe.committed
    );
    let wide: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: wide 以 NUL 结尾，生命周期覆盖调用。
    unsafe { SetWindowTextW(hwnd, wide.as_ptr()) };
}

/// 取窗口关联的 Probe。
///
/// SAFETY: 调用方须保证 GWLP_USERDATA 里确为 `Box<Probe>` 的裸指针（本文件内只有
/// WM_NCCREATE 写入一次，WM_DESTROY 释放）。
unsafe fn probe_of(hwnd: HWND) -> Option<&'static mut Probe> {
    // SAFETY: 见函数契约 —— GWLP_USERDATA 由 WM_NCCREATE 写入 Box::into_raw 的指针，
    // 并在 WM_DESTROY 回收，其间不会被替换为空或他物。
    let raw = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut Probe;
    if raw.is_null() {
        None
    } else {
        // SAFETY: raw 非空且指向仍存活的 Probe（回收发生在 WM_DESTROY 之后）。
        Some(unsafe { &mut *raw })
    }
}

/// 窗口过程。
unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // 整个消息处理体都在一个 unsafe 块内：这是 Win32 窗口过程的固有性质（句柄、
    // 裸指针、FFI 调用都来自 unsafe 边界）。逐条加 unsafe 只会淹没真正的风险点，
    // 因此这里统一声明一次，各处的 SAFETY 注释仍然逐点标注。
    unsafe {
        match msg {
        // 首次创建时把 Probe 挂上，并立即关联 IME 上下文。
        windows_sys::Win32::UI::WindowsAndMessaging::WM_NCCREATE => {
            let boxed = Box::new(Probe::new());
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(boxed) as isize);

            // 显式关联 IME 上下文 —— 探针要点之一：确认这一步能成功。
            let himc = ImmGetContext(hwnd);
            if himc.is_null() {
                eprintln!("[probe] ImmGetContext 失败，GetLastError={}", GetLastError());
            } else {
                println!("[probe] ImmGetContext 成功: {himc:?}");
                ImmReleaseContext(hwnd, himc);
            }
            // WM_NCCREATE 必须返回 TRUE(1)，否则系统认为窗口创建失败：
            // CreateWindowExW 会返回 NULL 并销毁窗口，进程随即退出。
            1
        }

        WM_IME_SETCONTEXT => {
            // 不吞掉：让 IMM32 照常处理候选窗。
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }

        WM_IME_STARTCOMPOSITION => {
            if let Some(p) = probe_of(hwnd) {
                p.ime_msg_count += 1;
                p.composing.clear();
                println!("[probe] WM_IME_STARTCOMPOSITION  #{}(组合开始)", p.ime_msg_count);
                refresh_title(hwnd, p);
            }
            0
        }

        WM_IME_COMPOSITION => {
            let himc = ImmGetContext(hwnd);
            if himc.is_null() {
                return DefWindowProcW(hwnd, msg, wparam, lparam);
            }

            // 组合串：拼音输入过程（如 "zhong"）。
            if lparam as u32 & GCS_COMPSTR != 0 {
                if let Some(s) = get_composition_string(himc, GCS_COMPSTR) {
                    if let Some(p) = probe_of(hwnd) {
                        p.ime_msg_count += 1;
                        p.composing = s.clone();
                        println!("[probe] GCS_COMPSTR  = {s:?}");
                    }
                }
            }

            // 光标位置：候选窗定位依据。
            if lparam as u32 & GCS_CURSORPOS != 0 {
                let cur = ImmGetCompositionStringW(himc, GCS_CURSORPOS, null_mut(), 0);
                if let Some(p) = probe_of(hwnd) {
                    p.cursor_pos = cur;
                    println!("[probe] GCS_CURSORPOS = {cur}");
                }
            }

            // 提交串：选字完成后的中文结果 —— 探针的核心验证点。
            if lparam as u32 & GCS_RESULTSTR != 0 {
                if let Some(s) = get_composition_string(himc, GCS_RESULTSTR) {
                    if let Some(p) = probe_of(hwnd) {
                        p.ime_msg_count += 1;
                        p.committed.push_str(&s);
                        p.composing.clear();
                        println!("[probe] GCS_RESULTSTR = {s:?}   <= 中文提交成功");
                    }
                }
            }

            // 把候选窗定位到客户区光标处 —— 验证 ImmSetCompositionWindow 通路。
            let mut rect = zeroed();
            if GetClientRect(hwnd, &mut rect) != 0 {
                let cf = COMPOSITIONFORM {
                    dwStyle: CFS_POINT,
                    ptCurrentPos: POINT {
                        x: 24,
                        y: (rect.bottom - 48).max(24),
                    },
                    rcArea: zeroed(),
                };
                // SAFETY: cf 是栈上有效值，调用期间存活。
                ImmSetCompositionWindow(himc, &cf);
            }

            ImmReleaseContext(hwnd, himc);

            if let Some(p) = probe_of(hwnd) {
                refresh_title(hwnd, p);
            }
            InvalidateRect(hwnd, null(), 0);
            0
        }

        WM_IME_ENDCOMPOSITION => {
            if let Some(p) = probe_of(hwnd) {
                p.ime_msg_count += 1;
                p.composing.clear();
                println!("[probe] WM_IME_ENDCOMPOSITION  #{}(组合结束)", p.ime_msg_count);
                refresh_title(hwnd, p);
            }
            0
        }

        WM_IME_CHAR => {
            if let Some(p) = probe_of(hwnd) {
                p.ime_msg_count += 1;
                let ch = char::from_u32(wparam as u32).unwrap_or('?');
                p.committed.push(ch);
                println!("[probe] WM_IME_CHAR = {ch:?}");
                refresh_title(hwnd, p);
            }
            0
        }

        windows_sys::Win32::UI::WindowsAndMessaging::WM_PAINT => {
            // 用 GDI 画两行提示，确认客户区可绘制（后续会换成 kanesumi-canvas）。
            use windows_sys::Win32::Graphics::Gdi::{
                BeginPaint, EndPaint, SelectObject, SetBkMode, SetTextColor, TextOutW,
                PAINTSTRUCT, TRANSPARENT,
            };

            let mut ps: PAINTSTRUCT = zeroed();
            let hdc = BeginPaint(hwnd, &mut ps);
            if !hdc.is_null() {
                SelectObject(hdc, GetStockObject(DEFAULT_GUI_FONT) as HFONT);
                SetBkMode(hdc, TRANSPARENT as i32);
                SetTextColor(hdc, rgb(240, 240, 240));

                let hint: Vec<u16> = "请用微软拼音在此窗口输入中文"
                    .encode_utf16()
                    .collect();
                TextOutW(hdc, 24, 24, hint.as_ptr(), hint.len() as i32);

                if let Some(p) = probe_of(hwnd) {
                    let line = format!("组合: {}   已提交: {}", p.composing, p.committed);
                    let w: Vec<u16> = line.encode_utf16().collect();
                    TextOutW(hdc, 24, 56, w.as_ptr(), w.len() as i32);
                }
                EndPaint(hwnd, &ps);
            }
            0
        }

        WM_CLOSE => {
            DestroyWindow(hwnd);
            0
        }

        WM_DESTROY => {
            let raw = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Probe;
            if !raw.is_null() {
                // SAFETY: 指针由 WM_NCCREATE 的 Box::into_raw 产生，且只在此处回收一次。
                let p = Box::from_raw(raw);
                println!("\n========== 探针结果 ==========");
                println!("IME 消息总数   : {}", p.ime_msg_count);
                println!("最终提交文本   : {:?}", p.committed);
                println!(
                    "判定           : {}",
                    if p.ime_msg_count > 0 {
                        "IME 通路畅通 ✓"
                    } else {
                        "未收到任何 IME 消息 ✗"
                    }
                );
                println!("=============================\n");
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            }
            PostQuitMessage(0);
            0
        }

        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

fn main() {
    println!("Arc Deck · IME 探针（Win32 + IMM32，无 GUI 框架）");
    println!("------------------------------------------------");

    // SAFETY: 以下整段是标准 Win32 启动序列；所有句柄都在使用前判空。
    unsafe {
        let hinstance = GetModuleHandleW(null());
        if hinstance.is_null() {
            eprintln!("GetModuleHandleW 失败: {}", GetLastError());
            std::process::exit(1);
        }

        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wnd_proc),
            hInstance: hinstance,
            hCursor: LoadCursorW(null_mut(), IDC_IBEAM),
            lpszClassName: CLASS_NAME.as_ptr(),
            ..zeroed()
        };

        if RegisterClassExW(&wc) == 0 {
            eprintln!("RegisterClassExW 失败: {}", GetLastError());
            std::process::exit(1);
        }

        let hwnd = CreateWindowExW(
            0,
            CLASS_NAME.as_ptr(),
            CLASS_NAME.as_ptr(),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            720,
            300,
            null_mut(),
            null_mut(),
            hinstance,
            null(),
        );

        if hwnd.is_null() {
            eprintln!("CreateWindowExW 失败: {}", GetLastError());
            std::process::exit(1);
        }

        ShowWindow(hwnd, SW_SHOW);
        UpdateWindow(hwnd);

        println!("窗口已创建。请用微软拼音输入中文，关闭窗口查看结论。\n");

        let mut msg: MSG = zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
