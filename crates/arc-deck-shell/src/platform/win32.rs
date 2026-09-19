// Arc Deck · Win32 FFI 绑定点
//
// **全仓唯一允许出现裸 FFI 声明的地方。** 把 Win32 的表面收敛到一个文件里，
// 是为了让上层的消息泵逻辑保持可读：`mod.rs` 里只应该有事件翻译与生命周期。
//
// 不用 `winit` —— 这是刻意的架构选择。egui 的 macOS IME 至今卡在
// `rust-windowing/winit#4432`（PR 自己的描述是 "was buggy, still buggy"），
// iced 输入 3 个中文即 panic 且修复 PR 挂了半年。根因不是那些框架写得差，
// 而是 **IME 归 winit 管，框架只能等**。
//
// Kanesumi 的 Linux 外壳用 `smithay-client-toolkit` + `calloop` 自研事件循环，
// 不用 winit。Windows 侧照此办理：自己写消息泵，IME 完全归自己管。

use windows_sys::Win32::UI::Input::Ime::HIMC;

pub use windows_sys::Win32::Foundation::{GetLastError, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
pub use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DeleteObject, EndPaint, FillRect as GdiFillRect, GetStockObject,
    InvalidateRect, SelectObject, SetBkMode, SetTextColor, TextOutW, UpdateWindow,
    DEFAULT_GUI_FONT, HDC, HFONT, PAINTSTRUCT, TRANSPARENT,
};
pub use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
pub use windows_sys::Win32::UI::Input::Ime::{
    ImmGetCompositionStringW, ImmGetContext, ImmReleaseContext, ImmSetCompositionWindow,
    CFS_POINT, COMPOSITIONFORM, GCS_COMPSTR, GCS_CURSORPOS, GCS_RESULTSTR,
};
pub use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect,
    GetMessageW, GetWindowLongPtrW, LoadCursorW, PostQuitMessage, RegisterClassExW,
    SetWindowLongPtrW, SetWindowTextW, ShowWindow, TranslateMessage, CS_HREDRAW, CS_VREDRAW,
    CW_USEDEFAULT, DLGC_WANTALLKEYS, DLGC_WANTARROWS, DLGC_WANTCHARS, GWLP_USERDATA, IDC_ARROW,
    IDC_IBEAM, MSG, SW_SHOW, WM_CHAR, WM_CLOSE, WM_DESTROY, WM_ERASEBKGND, WM_GETDLGCODE,
    WM_IME_CHAR, WM_IME_COMPOSITION, WM_IME_ENDCOMPOSITION, WM_IME_SETCONTEXT,
    WM_IME_STARTCOMPOSITION, WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL,
    WM_NCCREATE, WM_PAINT, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SETFOCUS, WM_SIZE, WNDCLASSEXW,
    WS_OVERLAPPEDWINDOW,
};

/// `RGB(r,g,b)` 是 Gdi 头文件里的 C 宏，windows-sys 不导出 —— 按其定义手写。
pub const fn rgb(r: u8, g: u8, b: u8) -> u32 {
    (r as u32) | ((g as u32) << 8) | ((b as u32) << 16)
}

/// 窗口过程签名。
pub type WndProc = unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT;

/// IME 上下文句柄的别名 —— 让上层不必 import IMM32 类型。
pub type ImeContext = HIMC;

/// `WM_KEYDOWN` 的 `wparam` 是虚拟键码；枚举用得上的那几个。
pub mod vk {
    pub const BACK: u32 = 0x08;
    pub const TAB: u32 = 0x09;
    pub const RETURN: u32 = 0x0D;
    pub const ESCAPE: u32 = 0x1B;
    pub const PRIOR: u32 = 0x21; // Page Up
    pub const NEXT: u32 = 0x22; // Page Down
    pub const END: u32 = 0x23;
    pub const HOME: u32 = 0x24;
    pub const LEFT: u32 = 0x25;
    pub const UP: u32 = 0x26;
    pub const RIGHT: u32 = 0x27;
    pub const DOWN: u32 = 0x28;
    pub const DELETE: u32 = 0x2E;
}
