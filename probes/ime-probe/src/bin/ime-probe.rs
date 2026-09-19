// Arc Deck · IME 组合验证（真实输入队列）
//
// 与 input-probe 的分工：
//
//   input-probe   —— PostMessage(WM_CHAR)，**绕过输入队列**。验证外壳的
//                    消息翻译与提交路径。确定性、可反复运行。
//   ime-probe（本工具）—— SendInput 真实键盘事件，**经过系统输入队列**，
//                    因此会被 IME 接管。这是唯一能自动触达「拼音组合」的途径。
//
// 为什么需要两条：IME 的组合发生在输入队列层（IMM32 在队列上工作），
// PostMessage 绕过队列因而驱动不了它。两者合起来才覆盖完整链路。
//
// 局限（必须说清）：本工具能触发组合，但**无法合成「选字提交」**——
// 候选列表的选择由 IME 自身完成。因此判据是「composing 是否变为非空」。

use std::thread::sleep;
use std::time::Duration;

const CLASS_NAME: &[u16] = &[
    b'A' as u16, b'r' as u16, b'c' as u16, b'D' as u16, b'e' as u16, b'c' as u16, b'k' as u16,
    b'S' as u16, b'h' as u16, b'e' as u16, b'l' as u16, b'l' as u16, 0,
];

#[repr(C)]
#[derive(Clone, Copy)]
struct KeybdInput {
    vk: u16,
    scan: u16,
    flags: u32,
    time: u32,
    extra: usize,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Input {
    kind: u32,
    _pad0: u32,
    ki: KeybdInput,
    _pad1: [u8; 8],
}

const INPUT_KEYBOARD: u32 = 1;
const KEYEVENTF_KEYUP: u32 = 0x0002;

#[link(name = "user32")]
unsafe extern "system" {
    fn FindWindowW(class: *const u16, title: *const u16) -> isize;
    fn SetForegroundWindow(hwnd: isize) -> i32;
    fn GetForegroundWindow() -> isize;
    fn SendInput(count: u32, inputs: *const Input, size: i32) -> u32;
    fn GetWindowTextW(hwnd: isize, buf: *mut u16, max: i32) -> i32;
    fn GetWindowThreadProcessId(hwnd: isize, pid: *mut u32) -> u32;
    fn AttachThreadInput(attach: u32, attach_to: u32, do_attach: i32) -> i32;
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetCurrentThreadId() -> u32;
}

/// 强制把目标窗口带到前台。
///
/// Windows 有「前台锁」：非前台进程调用 `SetForegroundWindow` 会被静默拒绝
/// （实测返回后前台仍是别人）。标准绕法是**附加到当前前台窗口的输入队列**
/// （`AttachThreadInput`），使自己被视为同一输入上下文，从而获得置前许可。
/// 附加与解附必须成对 —— 附着状态跨调用残留会干扰别的进程的输入处理。
///
/// 这是**测试基建**的需要，不是产品代码：产品的窗口由系统正常激活路径打开。
unsafe fn force_foreground(hwnd: isize) -> bool {
    // SAFETY: 标准 Win32 调用序列；每处 AttachThreadInput 都有配对的解附。
    unsafe {
        let me = GetCurrentThreadId();
        let fg = GetForegroundWindow();

        // 收集需要解附的线程，避免遗漏。
        let mut attached: Vec<u32> = Vec::new();
        for target in [fg, hwnd] {
            if target == 0 {
                continue;
            }
            let tid = GetWindowThreadProcessId(target, std::ptr::null_mut());
            if tid != 0 && tid != me && !attached.contains(&tid) {
                if AttachThreadInput(me, tid, 1) != 0 {
                    attached.push(tid);
                }
            }
        }

        let _ = SetForegroundWindow(hwnd);
        sleep(Duration::from_millis(350));
        let ok = GetForegroundWindow() == hwnd;

        for tid in attached {
            AttachThreadInput(me, tid, 0);
        }
        ok
    }
}

fn read_title(hwnd: isize) -> String {
    let mut buf = [0u16; 512];
    // SAFETY: buf 尺寸由常量给出，函数按 NUL 截断。
    let n = unsafe { GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
    if n <= 0 {
        return String::new();
    }
    String::from_utf16_lossy(&buf[..n as usize])
}

fn field<'a>(title: &'a str, key: &str) -> &'a str {
    title
        .split(&format!("{key}="))
        .nth(1)
        .map(|s| s.split_whitespace().next().unwrap_or(""))
        .unwrap_or("")
}

/// 发送一次虚拟键按下 + 抬起。
fn tap_vk(vk: u16) -> u32 {
    let mut total = 0u32;
    for flags in [0u32, KEYEVENTF_KEYUP] {
        let inp = Input {
            kind: INPUT_KEYBOARD,
            _pad0: 0,
            ki: KeybdInput {
                vk,
                scan: 0,
                flags,
                time: 0,
                extra: 0,
            },
            _pad1: [0; 8],
        };
        // SAFETY: inp 为栈上值，尺寸由 size_of 给出。
        let sent = unsafe { SendInput(1, &inp, std::mem::size_of::<Input>() as i32) };
        total += sent;
        sleep(Duration::from_millis(80));
    }
    total
}

fn main() {
    println!("Arc Deck · IME 组合验证（SendInput 真实输入队列）");
    println!("================================================");

    // SAFETY: 标准 Win32 查询；句柄使用前判空。
    let hwnd = unsafe { FindWindowW(CLASS_NAME.as_ptr(), std::ptr::null()) };
    if hwnd == 0 {
        eprintln!("未找到 Arc Deck 窗口 —— 请先启动 arc-deck-demo.exe");
        std::process::exit(1);
    }
    println!("目标窗口: {hwnd:#x}");

    // SAFETY: 先尝试常规置前，失败则用 AttachThreadInput 绕前台锁。
    let direct = unsafe { SetForegroundWindow(hwnd) } != 0;
    sleep(Duration::from_millis(300));
    let mut foreground_ok = unsafe { GetForegroundWindow() } == hwnd;
    if !foreground_ok {
        println!("常规置前未成功（前台锁），改用 AttachThreadInput…");
        foreground_ok = unsafe { force_foreground(hwnd) };
    }
    let fg = unsafe { GetForegroundWindow() };
    println!(
        "前台确认: {} (direct={direct} fg={fg:#x})",
        if foreground_ok { "是 ✓" } else { "否 ✗ —— 注入会落空" }
    );

    let before = read_title(hwnd);
    println!("注入前: {before}");

    // `zhong` 的虚拟键码：Z H O N G
    let keys: [u16; 5] = [0x5A, 0x48, 0x4F, 0x4E, 0x47];
    println!("\n注入 z-h-o-n-g（经输入队列，应被 IME 接管）…");
    let mut sent_total = 0u32;
    for k in keys {
        sent_total += tap_vk(k);
        // 每键之后读一次，观察组合是否逐步推进。
        let t = read_title(hwnd);
        println!("  发送 vk={k:#04X}  composing={:?}", field(&t, "composing"));
    }
    println!("SendInput 累计送达事件数: {sent_total}");

    sleep(Duration::from_millis(600));
    let after = read_title(hwnd);
    println!("\n注入后: {after}");

    let composing = field(&after, "composing");
    let app_events = field(&after, "app_events");

    println!("\n========== 判定 ==========");
    if !foreground_ok {
        println!("未能取得前台 —— 本测试无效，需在无人打扰的桌面上重跑 ✗");
    } else if !composing.is_empty() {
        println!("composing = {composing:?} —— **IME 组合已进入自研事件循环** ✓");
        println!("这解除了本项目的否决性风险：中文 IME 通路成立。");
    } else if app_events != "0" {
        println!("app_events = {app_events}（有输入到达，但未经 IME 组合）⚠");
        println!("可能 IME 处于英文模式，或未安装中文输入法。");
    } else {
        println!("composing 为空且 app_events=0 —— 注入未到达窗口 ✗");
        println!("常见原因：窗口未取得前台（UIPI 隔离）、或注入进程权限低于目标。");
    }
    println!("==========================");
    println!("\n局限：本工具能触发组合，但**无法合成「选字提交」**——候选列表的");
    println!("选择由 IME 自身完成。故判据是 composing 变为非空，而非 committed 增长。");
}
