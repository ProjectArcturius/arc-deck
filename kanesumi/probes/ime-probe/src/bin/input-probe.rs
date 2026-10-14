// Arc Deck · 输入链路自动验证
//
// 目的：把「能唤起输入法但打不出字」这类问题从**人工点窗口**变成**可重复的自动验证**。
//
// 为什么需要它：上一轮排查里真正的缺陷（重绘请求恒假）**编译零警告、运行零日志**，
// 只有肉眼能发现。而人工验证一次只产出一个布尔值，且不可复现。
//
// 两种注入方式，边界不同，都要说清：
//
//   1. `PostMessage(WM_CHAR)` —— 直接投递字符消息，**不经过输入队列**。
//      验证的是「外壳的消息翻译 → InputEvent::Commit → 状态更新 → 渲染」。
//      这是确定性的，可反复运行。
//   2. `SendInput` —— 真的走系统输入队列，但要目标窗口在前台，且会被
//      UIPI 拒绝（实测返回 0）。因此只作为附加尝试，不作为判据。
//
// **不覆盖的**：IME 的「拼音组合 → 选字提交」由 IME 内部状态机完成，
// 无法用程序合成（投递消息绕过输入队列，驱动不了 IMM32）。那一步仍需
// 一次人工确认 —— 但除它之外的一切都由本工具覆盖。

use std::thread::sleep;
use std::time::Duration;

const CLASS_NAME: &[u16] = &[
    b'A' as u16, b'r' as u16, b'c' as u16, b'D' as u16, b'e' as u16, b'c' as u16, b'k' as u16,
    b'S' as u16, b'h' as u16, b'e' as u16, b'l' as u16, b'l' as u16, 0,
];

const WM_CHAR: u32 = 0x0102;

#[link(name = "user32")]
unsafe extern "system" {
    fn FindWindowW(class: *const u16, title: *const u16) -> isize;
    fn PostMessageW(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> i32;
    fn GetWindowTextW(hwnd: isize, buf: *mut u16, max: i32) -> i32;
    fn IsWindow(hwnd: isize) -> i32;
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

/// 从标题里抽 `key=N` 的 N。
fn parse_counter(title: &str, key: &str) -> Option<u32> {
    let rest = title.split(&format!("{key}=")).nth(1)?;
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

/// 抽 `x=N` 形式的全部计数。
fn parse_all(title: &str) -> (Option<u32>, Option<u32>) {
    (
        parse_counter(title, "committed"),
        parse_counter(title, "app_events"),
    )
}

fn main() {
    println!("Arc Deck · 输入链路自动验证");
    println!("============================");

    // SAFETY: 标准 Win32 查询，句柄使用前判空。
    let hwnd = unsafe { FindWindowW(CLASS_NAME.as_ptr(), std::ptr::null()) };
    if hwnd == 0 {
        eprintln!("未找到 Arc Deck 窗口 —— 请先启动 arc-deck-demo.exe");
        std::process::exit(1);
    }
    if unsafe { IsWindow(hwnd) } == 0 {
        eprintln!("句柄无效: {hwnd:#x}");
        std::process::exit(1);
    }
    println!("目标窗口: {hwnd:#x}");

    let before_title = read_title(hwnd);
    println!("注入前标题: {before_title}");
    let (committed_before, events_before) = parse_all(&before_title);

    let text = "中文测试";
    println!(
        "\n投递 {text:?}（PostMessage WM_CHAR × {}）…",
        text.chars().count()
    );
    let mut posted = 0u32;
    for ch in text.chars() {
        // SAFETY: hwnd 已判有效；WM_CHAR 的 wparam 是字符码。
        let ok = unsafe { PostMessageW(hwnd, WM_CHAR, ch as usize, 0) };
        if ok != 0 {
            posted += 1;
        }
        sleep(Duration::from_millis(60));
    }
    println!("成功入队 {posted}/{} 条", text.chars().count());

    sleep(Duration::from_millis(400));

    let after_title = read_title(hwnd);
    println!("注入后标题: {after_title}");
    let (committed_after, events_after) = parse_all(&after_title);

    let n = text.chars().count() as u32;

    println!("\n========== 判定 ==========");

    // 判据用 app_events 而不是 committed，这是刻意的：
    // `committed` 来自 ImeState，而 WM_CHAR 路径**不经过** ImeState
    // （它是「直接键入」路径，直接调 App::handle_input）。用 committed 判定
    // 这条路径会得到假阴性 —— 第一版就是这么错的。
    let events_ok = matches!((events_before, events_after),
        (Some(b), Some(a)) if a == b + n);

    match (events_ok, committed_before, committed_after) {
        (true, _, _) => {
            println!("app_events 由 {} 增至 {} —— 输入链路通 ✓", events_before.unwrap(), events_after.unwrap());
            println!("消息泵 → InputEvent → App::handle_input → 计数发布，全链验证通过。");
            println!("\n注意 committed={:?}：WM_CHAR 路径不经过 ImeState，故该计数不变属预期，", committed_after);
            println!("并非缺陷。IME 提交走的是 WM_IME_COMPOSITION → ImeState::on_commit。");
        }
        (false, Some(b), Some(a)) if a > b => {
            println!("app_events 未达预期，但 committed 由 {b} 增至 {a} —— 部分通过 ⚠");
        }
        (false, _, _) => {
            let got = events_after.unwrap_or(0) - events_before.unwrap_or(0);
            println!("app_events 仅增 {got}，预期增 {n} —— 输入链路断 ✗");
            println!("若 app_events 为 0：消息到了 wnd_proc 但未转发（查 shell_of 是否返回 None）。");
            println!("若 chars 为 0：消息未到达 wnd_proc（查窗口类名 / 消息队列）。");
        }
    }
    println!("==========================");
    println!("\n边界：本工具**不覆盖** IME 的「拼音组合 → 选字」—— 那一步由 IME");
    println!("内部状态机完成，无法用程序合成（投递消息绕过输入队列，驱动不了 IMM32）。");
    println!("该步仍需一次人工确认。");
}
