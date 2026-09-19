// Arc Deck · 流式文本排版性能探针
//
// 要回答的问题（它直接决定渲染管线怎么设计，晚知道会返工）：
//
//   Agent GUI 的性能瓶颈不是绘制，是**文本排版**。流式追加时每个 token 都会
//   改变消息内容，而排版缓存的键是内容派生的 —— 于是缓存永远命中不了，
//   整条消息每来一个 token 就要重新塑形/换行/光栅化。
//
// 一条 2 万字符的助手回复，如果每次追加都要全量重排，这个代价就是天花板。
//
// 本探针把代价拆成三段分别计时，因为三段的优化手段完全不同：
//
//   1. 塑形（rustybuzz）—— 与内容长度线性相关，纯计算
//   2. 断行（unicode-linebreak）—— 线性相关，需要字体度量才能定宽
//   3. 光栅化（fontdue）—— **只与「新出现的可见字形」相关**，可缓存
//
// 关键判断依据：若第 3 段占绝对多数，则优化方向是**字形缓存 + 只重排尾部段落**；
// 若第 1 段占多数，则方向是**按段落切分做增量塑形**。

use std::time::Instant;

/// 造一段接近真实的中文长回复：中英混排 + 标点。
fn build_message(target_chars: usize) -> String {
    let chunk = "DeepSeek Harness 是一个全插件的 agent 运行时。它的核心设计是\
把 host 与 client 能力分层，通道无关的 RPC 协议位于两者之间。\
这意味着客户端可以替换自己的载体，而不必改动协议本身。\
实现时需要注意：组合串不进入文档，提交串是唯一的写入路径。";
    let mut s = String::with_capacity(target_chars * 3);
    while s.chars().count() < target_chars {
        s.push_str(chunk);
        s.push('\n');
    }
    s
}

fn main() {
    // 优先用单体 TTF：fontdue 对 TTC 集合需要额外指定 face index。
    let candidates = [
        r"C:\Windows\Fonts\simhei.ttf",
        r"C:\Windows\Fonts\Deng.ttf",
    ];
    let font_path = candidates
        .iter()
        .find(|p| std::path::Path::new(p).exists())
        .copied()
        .unwrap_or_else(|| {
            eprintln!("未找到可用 CJK 字体，请改 font_path");
            std::process::exit(1);
        });

    println!("Arc Deck · 流式文本排版性能探针");
    println!("================================");
    println!("字体: {font_path}");

    let bytes = match std::fs::read(font_path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("读字体失败: {e}");
            std::process::exit(1);
        }
    };
    println!("字体大小: {:.1} MB", bytes.len() as f64 / 1_048_576.0);

    let t = Instant::now();
    let font = fontdue::Font::from_bytes(bytes.as_slice(), fontdue::FontSettings::default())
        .expect("字体解析失败");
    println!("字体解析耗时: {:.1} ms\n", t.elapsed().as_secs_f64() * 1000.0);

    let t = Instant::now();
    let face = rustybuzz::Face::from_slice(bytes.as_slice(), 0).expect("rustybuzz 解析失败");
    println!("rustybuzz 解析耗时: {:.1} ms\n", t.elapsed().as_secs_f64() * 1000.0);

    // 仅解析一次，下面每轮复用 —— 真实应用也是解析一次常驻。
    let _ = &font;
    let _ = &face;

    for target in [2_000usize, 20_000] {
        let msg = build_message(target);
        let chars = msg.chars().count();
        let lines = msg.lines().count();
        println!("── 消息规模: {} 字符 / {} 行 ──", chars, lines);

        // 1 · 断行：UAX #14
        let t = Instant::now();
        let mut breaks = 0usize;
        for _ in 0..20 {
            breaks = unicode_linebreak::linebreaks(&msg).count();
        }
        let per = t.elapsed().as_secs_f64() * 1000.0 / 20.0;
        println!("  断行 (UAX#14)      : {per:8.3} ms  ({breaks} 个断点)");

        // 2 · 塑形：rustybuzz 全量
        let t = Instant::now();
        let mut glyph_total = 0usize;
        const SHAPE_RUNS: usize = 20;
        for _ in 0..SHAPE_RUNS {
            let mut buffer = rustybuzz::UnicodeBuffer::new();
            buffer.push_str(&msg);
            let out = rustybuzz::shape(&face, &[], buffer);
            glyph_total = out.glyph_infos().len();
        }
        let per = t.elapsed().as_secs_f64() * 1000.0 / SHAPE_RUNS as f64;
        println!("  塑形 (rustybuzz)   : {per:8.3} ms  ({glyph_total} 个字形)");

        // 3 · 光栅化：fontdue，模拟「首次见到这批字形」
        //
        // 这是唯一可缓存的段。真实场景里稳态下每个 token 只引入 0-1 个新字形，
        // 因此稳态成本 ≈ 一次查表；这里测的是**冷启动最坏情况**（全部新字形）。
        let t = Instant::now();
        let mut rasterized = 0usize;
        let mut pixels = 0usize;
        for ch in msg.chars() {
            if ch.is_whitespace() {
                continue;
            }
            let (metrics, bitmap) = font.rasterize(ch, 15.0);
            rasterized += 1;
            pixels += bitmap.len();
            let _ = metrics;
        }
        let cold = t.elapsed().as_secs_f64() * 1000.0;
        println!("  光栅化 (fontdue)   : {cold:8.3} ms  ({rasterized} 个字形, {pixels} 像素) —— 冷启动全量");

        // 稳态估算：每 token 只引入 ~1 个新字形。
        let per_glyph = cold / rasterized.max(1) as f64;
        println!("  └ 单个字形均摊     : {per_glyph:8.4} ms  → 稳态每 token ≈ {per_glyph:.4} ms");

        println!();
    }

    println!("================================");
    println!("判读：");
    println!("- 光栅化远大于塑形 ⇒ 优化方向是**字形缓存**（按 (char,size) 键），");
    println!("  稳态成本降到一次查表，与消息长度无关。");
    println!("- 塑形与消息长度同阶增长 ⇒ 需要**按段落切分做增量塑形**，");
    println!("  只重排被追加的尾段。");
    println!("- 断行同理：UAX#14 是线性扫描，应按段落缓存结果。");
}
