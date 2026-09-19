// Arc Deck · Win32 平台外壳
//
// 职责（与 Kanesumi `kanesumi-harness::platform` 的 Linux 实现同构）：
//
// 1. 创建窗口
// 2. 跑消息泵
// 3. 把 `WM_*` / `WM_IME_*` 翻译成平台无关的 [`InputEvent`]
// 4. 把 [`Scene`] 光栅化上屏
// 5. 中心化重绘门控（`App::needs_redraw`）
//
// 本文件只应有事件翻译与生命周期；裸 FFI 全部收敛在 [`win32`]。

pub mod win32;

use crate::app::{App, HeuristicTextEngine, TextEngine};
use crate::ime::ImeState;
use crate::input::{InputEvent, Key, Modifiers, PointerButton};
use crate::scene::{Color, Point, Rect, Scene, SceneCommand, Size, TextAlign};

use std::cell::RefCell;
use std::ffi::c_void;
use std::mem::zeroed;
use std::ptr::{null, null_mut};

use win32::*;

/// 窗口类名。
const CLASS_NAME: &[u16] = &[
    b'A' as u16, b'r' as u16, b'c' as u16, b'D' as u16, b'e' as u16, b'c' as u16, b'k' as u16,
    b'S' as u16, b'h' as u16, b'e' as u16, b'l' as u16, b'l' as u16, 0,
];

/// 外壳持有的运行时状态。
///
/// 用 `RefCell<ShellState>` + `Box::into_raw` 挂到 `GWLP_USERDATA`：Win32 的
/// 窗口过程是 C ABI 回调，没有闭包捕获，这是把 Rust 状态带进去的标准做法。
struct ShellState {
    app: Box<dyn App>,
    text: HeuristicTextEngine,
    ime: ImeState,
    /// 当前帧是否已在绘制中（`WM_PAINT` 置位、绘制完成后清除）。
    ///
    /// 注意：**这不是重绘请求**。绝不能拿它当「是否已排程重绘」用 ——
    /// 早期版本正是这么错的：`if needs_redraw() && !dirty` 里 `dirty` 已被
    /// 消息处理置位，于是 `InvalidateRect` 永远发不出去，表现为
    /// 「能唤起输入法但屏幕不更新」。
    painting: bool,
    client: Size,
}

impl ShellState {
    fn new(app: Box<dyn App>) -> Self {
        let cfg = app.config();
        Self {
            app,
            text: HeuristicTextEngine,
            ime: ImeState::new(),
            painting: false,
            client: Size::new(cfg.width, cfg.height),
        }
    }

    /// 排程一次重绘。任何改变可见状态的地方都必须调用它。
    ///
    /// # Safety
    /// `hwnd` 必须是本状态所属的窗口。
    unsafe fn request_repaint(&self, hwnd: HWND) {
        // SAFETY: 见函数契约。
        unsafe { InvalidateRect(hwnd, null(), 0) };
    }
}

/// 取得挂在窗口上的外壳状态。
///
/// # Safety
/// `hwnd` 必须是本模块创建、且已由 `WM_NCCREATE` 写入 `Box::into_raw` 指针的窗口。
unsafe fn shell_of(hwnd: HWND) -> Option<&'static mut ShellState> {
    // SAFETY: 见函数契约。
    let raw = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut ShellState;
    if raw.is_null() {
        None
    } else {
        // SAFETY: 非空且指向仍存活的状态（回收发生在 WM_DESTROY）。
        Some(unsafe { &mut *raw })
    }
}

/// 取一笔 IMM32 组合串。返回 `None` 表示该标志本次无内容。
///
/// # Safety
/// `himc` 必须是 `ImmGetContext` 取得的有效上下文，且调用方持有期间未被释放。
unsafe fn composition_string(himc: ImeContext, flag: u32) -> Option<String> {
    // SAFETY: 传空缓冲 + 0 长度是 IMM32 约定的「问长度」用法。
    let byte_len = unsafe { ImmGetCompositionStringW(himc, flag, null_mut(), 0) };
    if byte_len <= 0 {
        return None;
    }
    let chars = byte_len as usize / 2 + 1;
    let mut buf: Vec<u16> = vec![0; chars];
    // SAFETY: buf 至少有 byte_len 字节容量。
    let written = unsafe {
        ImmGetCompositionStringW(himc, flag, buf.as_mut_ptr() as *mut c_void, byte_len as u32)
    };
    if written <= 0 {
        return None;
    }
    buf.truncate(written as usize / 2);
    Some(String::from_utf16_lossy(&buf))
}

/// 把 Scene 光栅化到窗口。
///
/// 当前是 **GDI 参考实现**：它证明 Scene 契约是可渲染的，并且让骨架能立刻跑起来。
/// 它与 Kanesumi 的 `CpuRenderer` 同属「不依赖 GPU」的一档 —— 第一版外壳不需要
/// 先啃 wgpu（上游 `cpu_raster.rs` 就是一个纯 CPU 扫描线光栅器 + 4× 超采样，
/// 那条路是可移植的，只是目前被 Linux 门控挡住）。
///
/// 未实现的命令（`Arc` / `Triangle` / `Image`）在此**静默跳过**，并由调用方
/// 通过日志暴露 —— 明确留缺口，好过假装画了。
fn paint_scene(hwnd: HWND, scene: &Scene) {
    // SAFETY: 标准 GDI 绘制序列；所有句柄用前判空。
    unsafe {
        let mut ps: PAINTSTRUCT = zeroed();
        let hdc = BeginPaint(hwnd, &mut ps);
        if hdc.is_null() {
            return;
        }

        let mut skipped = 0u32;

        for cmd in &scene.commands {
            match cmd {
                SceneCommand::FillRect { rect, color } => {
                    fill(hdc, rect, color);
                }

                SceneCommand::StrokeRect {
                    rect,
                    color,
                    width,
                } => {
                    // 描边向内：与正典一致，描边不侵占相邻元素的像素。
                    let w = width.max(1.0);
                    fill(hdc, &Rect::new(rect.x, rect.y, rect.w, w), color); // 上
                    fill(hdc, &Rect::new(rect.x, rect.bottom() - w, rect.w, w), color); // 下
                    fill(hdc, &Rect::new(rect.x, rect.y, w, rect.h), color); // 左
                    fill(hdc, &Rect::new(rect.right() - w, rect.y, w, rect.h), color); // 右
                }
                SceneCommand::PushClip { rect } => {
                    apply_clip(hdc, rect);
                }

                SceneCommand::PopClip => {
                    // 裁剪栈由 CPU 光栅化器在软件层实现（见 apply_clip 说明），
                    // 此处无状态可恢复。
                }

                SceneCommand::Text {
                    pos,
                    text,
                    size,
                    color,
                    align,
                } => {
                    draw_text(hdc, *pos, text, *size, *color, *align);
                }

                SceneCommand::Arc { .. }
                | SceneCommand::Triangle { .. }
                | SceneCommand::Image { .. } => {
                    skipped += 1;
                }
            }
        }

        EndPaint(hwnd, &ps);

        if skipped > 0 {
            // 只在有缺口时提示一次，避免刷屏。
            log_once(skipped);
        }
    }
}

thread_local! {
    /// 已提示过的未实现命令数 —— 避免每帧刷屏。
    static LOGGED_SKIP: RefCell<u32> = const { RefCell::new(0) };
}

fn log_once(skipped: u32) {
    LOGGED_SKIP.with(|c| {
        let mut seen = c.borrow_mut();
        if *seen != skipped {
            *seen = skipped;
            eprintln!("[arc-deck] GDI 渲染器暂未实现 {skipped} 条命令（Arc/Triangle/Image）");
        }
    });
}

/// 实心填充一个逻辑像素矩形。
///
/// # Safety
/// `hdc` 必须是有效的设备上下文。
unsafe fn fill(hdc: HDC, r: &Rect, color: &Color) {
    if r.is_empty() {
        return;
    }
    let (cr, cg, cb) = to_byte_rgb(color);
    // SAFETY: GDI 对象在函数末尾删除；RECT 为栈上值。
    unsafe {
        let brush = CreateSolidBrush(rgb(cr, cg, cb));
        if brush.is_null() {
            return;
        }
        let mut rc = RECT {
            left: r.x.round() as i32,
            top: r.y.round() as i32,
            right: r.right().round() as i32,
            bottom: r.bottom().round() as i32,
        };
        GdiFillRect(hdc, &mut rc, brush);
        DeleteObject(brush);
    }
}

/// 应用裁剪矩形。
///
/// **当前为空实现（仅记录语义）**：GDI 的裁剪需要 `SelectClipRgn` +
/// `CreateRectRgn` 的区域生命周期管理，在骨架阶段引入它会掩盖真正的问题。
/// 裁剪应当在软件光栅化器里实现 —— 那是更可控、更可测的路径
/// （上游 Kanesumi 的 `cpu_raster.rs` 就是一个纯 CPU 扫描线光栅器，
/// 含 4× 超采样，且它是**可移植的**，只是目前被 Linux 门控挡住）。
unsafe fn apply_clip(_hdc: HDC, _r: &Rect) {}

/// 画一段文本。
///
/// # Safety
/// `hdc` 必须有效。
unsafe fn draw_text(
    hdc: HDC,
    pos: Point,
    text: &str,
    size: f32,
    color: Color,
    align: TextAlign,
) {
    if text.is_empty() {
        return;
    }
    let (cr, cg, cb) = to_byte_rgb(&color);
    // SAFETY: 字体取自 stock object，不拥有所有权；字符串为栈上 UTF-16。
    unsafe {
        SelectObject(hdc, GetStockObject(DEFAULT_GUI_FONT) as HFONT);
        SetBkMode(hdc, TRANSPARENT as i32);
        SetTextColor(hdc, rgb(cr, cg, cb));

        let wide: Vec<u16> = text.encode_utf16().collect();
        let width = text.chars().count() as f32 * size * 0.9;
        let x = match align {
            TextAlign::Left => pos.x,
            TextAlign::Center => pos.x - width * 0.5,
            TextAlign::Right => pos.x - width,
        };
        TextOutW(
            hdc,
            x.round() as i32,
            pos.y.round() as i32,
            wide.as_ptr(),
            wide.len() as i32,
        );
    }
}

/// 线性 f32 通道 → 8 位。超范围夹紧。
fn to_byte_rgb(c: &Color) -> (u8, u8, u8) {
    let f = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    (f(c.r), f(c.g), f(c.b))
}

/// 窗口过程 —— 事件翻译的唯一入口。
unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // 整个窗口过程体在一个 unsafe 块内：这是 Win32 回调的固有性质。
    // 逐条加 unsafe 只会淹没真正的风险点，因此统一声明一次。
    unsafe {
        match msg {
            WM_NCCREATE => {
                // App 由 run() 在窗口创建前放入线程局部槽位，此处取出并接管。
                PENDING_APP.with(|p| {
                    if let Some(app) = p.borrow_mut().take() {
                        let state = Box::new(ShellState::new(app));
                        SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(state) as isize);
                    }
                });
                // WM_NCCREATE 必须返回 TRUE(1)，否则系统判定窗口创建失败：
                // CreateWindowExW 返回 NULL 并销毁窗口，进程随即退出。
                1
            }

            WM_SIZE => {
                if let Some(s) = shell_of(hwnd) {
                    let w = (lparam & 0xFFFF) as f32;
                    let h = ((lparam >> 16) & 0xFFFF) as f32;
                    s.client = Size::new(w, h);
                    s.request_repaint(hwnd);
                }
                0
            }

            WM_IME_SETCONTEXT => DefWindowProcW(hwnd, msg, wparam, lparam),

            WM_IME_STARTCOMPOSITION => {
                if let Some(s) = shell_of(hwnd) {
                    s.ime.on_composition_start();
                    s.request_repaint(hwnd);
                }
                // 交回系统：IME 在组合窗口上有自己的簿记，吞掉会导致
                // 组合无法推进（表现为「能唤起输入法但打不出字」）。
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }

            WM_IME_COMPOSITION => {
                let flags = lparam as u32;
                if let Some(s) = shell_of(hwnd) {
                    let himc = ImmGetContext(hwnd);
                    if !himc.is_null() {
                        // 组合串：仅更新显示，绝不进入文档。
                        if flags & GCS_COMPSTR != 0 {
                            if let Some(t) = composition_string(himc, GCS_COMPSTR) {
                                s.ime.on_composition_update(t, None);
                            }
                        }
                        if flags & GCS_CURSORPOS != 0 {
                            let cur = ImmGetCompositionStringW(himc, GCS_CURSORPOS, null_mut(), 0);
                            let text = s.ime.preedit().to_string();
                            s.ime.on_composition_update(text, Some(cur));
                        }
                        // 提交串：唯一进入文档的路径。
                        if flags & GCS_RESULTSTR != 0 {
                            if let Some(t) = composition_string(himc, GCS_RESULTSTR) {
                                if let Some(ev) = s.ime.on_commit(t) {
                                    s.app.handle_input(ev);
                                }
                            }
                        }

                        // 候选窗定位到插入点。App 决定插入点 —— 外壳不知道
                        // 文本框画在哪里。
                        let caret = s.app.ime_caret();
                        let cf = COMPOSITIONFORM {
                            dwStyle: CFS_POINT,
                            ptCurrentPos: POINT {
                                x: caret.x.round() as i32,
                                y: caret.y.round() as i32,
                            },
                            rcArea: zeroed(),
                        };
                        ImmSetCompositionWindow(himc, &cf);
                        ImmReleaseContext(hwnd, himc);
                    }
                    s.request_repaint(hwnd);
                }
                // 同上：必须交回系统，否则组合状态不推进。
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }

            WM_IME_ENDCOMPOSITION => {
                if let Some(s) = shell_of(hwnd) {
                    s.ime.on_composition_end();
                    s.request_repaint(hwnd);
                }
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }

            // 告诉系统「键盘输入都给我，包括 IME 要的」。
            // 缺这一条时，系统可能把按键吞在对话框管理器层面。
            WM_GETDLGCODE => {
                (DLGC_WANTALLKEYS | DLGC_WANTCHARS | DLGC_WANTARROWS) as LRESULT
            }

            // 焦点进入时必须让 IME 重新关联到本窗口 —— 不处理会表现为
            // 「输入法状态栏点亮了，但组合窗不跟着这个窗口走」。
            WM_SETFOCUS => {
                let himc = ImmGetContext(hwnd);
                if !himc.is_null() {
                    ImmReleaseContext(hwnd, himc);
                }
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }

            WM_PAINT => {
                if let Some(s) = shell_of(hwnd) {
                    let mut scene = Scene::new();
                    let client = s.client;
                    // 借出 TextEngine 的具体实现，再按 trait 对象传入 —— App 只
                    // 认识 `&dyn TextEngine`，将来换成 kanesumi-canvas 的 TextEngine
                    // 时这里改一行即可。
                    let engine: &dyn TextEngine = &s.text;
                    s.app.render(engine, client, &mut scene);
                    paint_scene(hwnd, &scene);
                    s.painting = false;
                } else {
                    // 状态尚未建立 —— 仍要走 BeginPaint/EndPaint 以满足系统要求。
                    let mut ps: PAINTSTRUCT = zeroed();
                    let hdc = BeginPaint(hwnd, &mut ps);
                    if !hdc.is_null() {
                        EndPaint(hwnd, &ps);
                    }
                }
                0
            }

            WM_ERASEBKGND => {
                // 我们自己画满客户区，系统擦除只会造成闪烁。
                1
            }

            WM_MOUSEMOVE => {
                let x = (lparam & 0xFFFF) as i16 as f32;
                let y = ((lparam >> 16) & 0xFFFF) as i16 as f32;
                if let Some(s) = shell_of(hwnd) {
                    s.app.handle_input(InputEvent::PointerMoved(Point::new(x, y)));
                    if s.app.needs_redraw() {
                        s.request_repaint(hwnd);
                    }
                }
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }

            WM_LBUTTONDOWN | WM_RBUTTONDOWN => {
                let button = if msg == WM_LBUTTONDOWN {
                    PointerButton::Left
                } else {
                    PointerButton::Right
                };
                let x = (lparam & 0xFFFF) as i16 as f32;
                let y = ((lparam >> 16) & 0xFFFF) as i16 as f32;
                if let Some(s) = shell_of(hwnd) {
                    s.app.handle_input(InputEvent::PointerPressed {
                        at: Point::new(x, y),
                        button,
                    });
                    s.request_repaint(hwnd);
                }
                0
            }

            WM_LBUTTONUP | WM_RBUTTONUP => {
                let button = if msg == WM_LBUTTONUP {
                    PointerButton::Left
                } else {
                    PointerButton::Right
                };
                let x = (lparam & 0xFFFF) as i16 as f32;
                let y = ((lparam >> 16) & 0xFFFF) as i16 as f32;
                if let Some(s) = shell_of(hwnd) {
                    s.app.handle_input(InputEvent::PointerReleased {
                        at: Point::new(x, y),
                        button,
                    });
                    s.request_repaint(hwnd);
                }
                0
            }

            WM_MOUSEWHEEL => {
                let delta = ((wparam >> 16) & 0xFFFF) as i16 as f32;
                // 一格滚轮 = 120；换算为逻辑像素（3 行 × 约 20px）。
                let dy = -delta / 120.0 * 60.0;
                if let Some(s) = shell_of(hwnd) {
                    let at = Point::new(0.0, 0.0);
                    s.app.handle_input(InputEvent::Scroll { at, dx: 0.0, dy });
                    s.request_repaint(hwnd);
                }
                0
            }

            WM_KEYDOWN => {
                if let Some(s) = shell_of(hwnd) {
                    // 组合期间（IME 已吞掉按键）不应把按键当文本 —— 这是
                    // 多数框架重复插入字符的根因。有组合串时直接交给 IME。
                    if !s.ime.is_composing() {
                        let key = match wparam as u32 {
                            vk::ESCAPE => Some(Key::Escape),
                            vk::RETURN => Some(Key::Enter),
                            vk::BACK => Some(Key::Backspace),
                            vk::DELETE => Some(Key::Delete),
                            vk::TAB => Some(Key::Tab),
                            vk::LEFT => Some(Key::Left),
                            vk::RIGHT => Some(Key::Right),
                            vk::UP => Some(Key::Up),
                            vk::DOWN => Some(Key::Down),
                            vk::HOME => Some(Key::Home),
                            vk::END => Some(Key::End),
                            vk::PRIOR => Some(Key::PageUp),
                            vk::NEXT => Some(Key::PageDown),
                            _ => None,
                        };
                        if let Some(k) = key {
                            s.app.handle_input(InputEvent::KeyPressed {
                                key: k,
                                mods: Modifiers::default(),
                            });
                            s.request_repaint(hwnd);
                        }
                    }
                }
                0
            }

            WM_CHAR | WM_IME_CHAR => {
                // 非 IME 路径的字符输入（直接键入 ASCII）。
                if let Some(ch) = char::from_u32(wparam as u32) {
                    // 过滤控制字符：回车/退格等已作为 Key 事件处理。
                    if !ch.is_control() {
                        if let Some(s) = shell_of(hwnd) {
                            s.app.handle_input(InputEvent::Commit {
                                text: ch.to_string(),
                            });
                            s.request_repaint(hwnd);
                        }
                    }
                }
                0
            }

            WM_CLOSE => {
                DestroyWindow(hwnd);
                0
            }

            WM_DESTROY => {
                let raw = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut ShellState;
                if !raw.is_null() {
                    // SAFETY: 指针由 WM_NCCREATE 的 Box::into_raw 产生，此处回收一次。
                    drop(Box::from_raw(raw));
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                }
                PostQuitMessage(0);
                0
            }

            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

thread_local! {
    /// 窗口创建前 App 的暂存槽 —— 因为 `WNDPROC` 是 C ABI，无法捕获闭包。
    static PENDING_APP: RefCell<Option<Box<dyn App>>> = const { RefCell::new(None) };
}

/// 运行应用 —— 阻塞直到窗口关闭。
///
/// 这是外壳的入口，对应 Kanesumi 的 `kanesumi_harness::platform::run`。
/// Ether 的 launcher 就是 `main.rs` 里一句 `platform::run(...)`。
pub fn run(app: Box<dyn App>) -> Result<(), String> {
    // SAFETY: 标准 Win32 启动序列；所有句柄使用前判空。
    unsafe {
        let hinstance = GetModuleHandleW(null());
        if hinstance.is_null() {
            return Err(format!("GetModuleHandleW 失败: {}", GetLastError()));
        }

        let cfg = app.config();

        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wnd_proc),
            hInstance: hinstance,
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            lpszClassName: CLASS_NAME.as_ptr(),
            ..zeroed()
        };

        if RegisterClassExW(&wc) == 0 {
            return Err(format!("RegisterClassExW 失败: {}", GetLastError()));
        }

        // App 在 CreateWindowExW 触发 WM_NCCREATE 之前入槽。
        PENDING_APP.with(|p| *p.borrow_mut() = Some(app));

        let title: Vec<u16> = cfg.title.encode_utf16().chain(std::iter::once(0)).collect();

        let hwnd = CreateWindowExW(
            0,
            CLASS_NAME.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            cfg.width as i32,
            cfg.height as i32,
            null_mut(),
            null_mut(),
            hinstance,
            null(),
        );

        if hwnd.is_null() {
            return Err(format!("CreateWindowExW 失败: {}", GetLastError()));
        }

        ShowWindow(hwnd, SW_SHOW);
        UpdateWindow(hwnd);

        // 消息泵。
        //
        // 重绘门控：`App::needs_redraw()` 为真时请求一次重绘 —— 这是
        // **中心化**的，所有动画都汇总到这一处判断，于是「新增动画却忘了
        // 请求重绘」在结构上不可能发生（PezMax 的实测教训：曾经因为逐个动画
        // 各自 request_repaint，一个 GIF 头像就让空闲时全屏 60fps 空转）。
        let mut msg: MSG = zeroed();
        loop {
            let ret = GetMessageW(&mut msg, null_mut(), 0, 0);
            if ret <= 0 {
                break; // 0 = WM_QUIT，-1 = 错误
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);

            if let Some(s) = shell_of(hwnd) {
                // App 自报需要重绘（动画在跑）且当前不在绘制中 —— 排程一帧。
                if s.app.needs_redraw() && !s.painting {
                    s.request_repaint(hwnd);
                }
            }
        }
    }
    Ok(())
}
