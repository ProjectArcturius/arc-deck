using System;
using System.Collections.Generic;
using System.Text;
using System.Text.RegularExpressions;
using Windows.UI.Text;
using Windows.UI.Xaml;
using Windows.UI.Xaml.Controls;
using Windows.UI.Xaml.Documents;
using Windows.UI.Xaml.Media;

namespace ArcDeck
{
    /// <summary>
    /// 把模型输出里的 Markdown 渲染成 TextBlock 的 inlines。
    ///
    /// ── 为什么需要它 ────────────────────────────────────────────────
    ///
    /// 消息原文里本来就有 Markdown（`**转发给 App**` 之类），而原先一律当纯文本
    /// 显示 —— 界面上直接出现一串星号，既难读又显得没做完。
    ///
    /// ── 为什么是「轻量 parser」而不是引入 Markdown 库 ───────────────
    ///
    /// 需要覆盖的只是**对话里实际出现的那几种**语法。引一个完整实现要处理
    /// 表格、脚注、嵌套引用、HTML 内联等一大堆用不到的东西，还要为它的
    /// 转义规则与边界情况负责。这里明确只支持：
    ///
    ///     行内    **粗体**   *斜体*   `行内代码`   [链接](url)
    ///     块级    # 标题（1–3 级）   - / * / + 无序列表   1. 有序列表
    ///             > 引用   --- 分隔线   空行分段
    ///
    /// **不支持的语法应当原样显示**，而不是被吃掉或渲染错 —— 这一点比多支持几种
    /// 语法重要：宁可让用户看到 `~~删除线~~`，也不要让它变成一段乱码。
    ///
    /// ── 为什么用附加属性 ────────────────────────────────────────────
    ///
    /// `TextBlock.Inlines` 不是 `x:Bind` 能直接绑定的目标类型，而
    /// `InlineCollection` 也不能当转换器的返回值。附加属性是 UWP 里做这件事的
    /// 常规写法：XAML 里写 `local:Markdown.Text="{x:Bind Text}"`，
    /// 属性变化回调里填充 inlines。
    /// </summary>
    public static class Markdown
    {
        /// <summary>Markdown 源文本。</summary>
        public static readonly DependencyProperty TextProperty =
            DependencyProperty.RegisterAttached(
                "Text",
                typeof(string),
                typeof(Markdown),
                new PropertyMetadata(null, OnTextChanged));

        public static string GetText(DependencyObject d) => (string)d.GetValue(TextProperty);

        public static void SetText(DependencyObject d, string value) => d.SetValue(TextProperty, value);

        /// <summary>
        /// 行内代码与代码块的字体。
        ///
        /// 写成常量而不是从资源里取：UWP 没有提供「等宽字体」令牌，
        /// 而 `Consolas` 从 Win7 起一直在，是最稳的选择。
        /// </summary>
        const string MonoFont = "Consolas";

        static void OnTextChanged(DependencyObject d, DependencyPropertyChangedEventArgs e)
        {
            // 目标必须是 RichTextBlock。`TextBlock.Inlines` 只接受 Inline，
            // 而本渲染器要用 Paragraph 表达块级间距 —— 那是 RichTextBlock 才有的。
            // 用错类型在编译期看不出来（附加属性挂哪个类型都合法），
            // 所以这里显式挡一道。
            if (d is not RichTextBlock rich)
            {
                return;
            }

            rich.Blocks.Clear();

            var source = e.NewValue as string;
            if (string.IsNullOrEmpty(source))
            {
                return;
            }

            Render(rich, source);
        }

        // ── 块级 ────────────────────────────────────────────────────────

        static void Render(RichTextBlock target, string source)
        {
            var lines = source.Replace("\r\n", "\n").Replace('\r', '\n').Split('\n');

            // 连续的无序/有序列表项要归成同一组，才能决定「首项前缀用 • 还是
            // 第几号」以及段间距。逐行独立处理会得到一堆各自成段的条目。
            var listBuffer = new List<string>();
            var listOrdered = false;

            void FlushList()
            {
                if (listBuffer.Count == 0)
                {
                    return;
                }

                for (var i = 0; i < listBuffer.Count; i++)
                {
                    var prefix = listOrdered ? $"{i + 1}. " : "• ";
                    var line = new Paragraph();

                    line.Inlines.Add(new Run { Text = prefix });
                    AppendInline(line, listBuffer[i]);

                    // 列表项之间用小一点的间距，让一组列表读起来是整体的。
                    line.Margin = new Thickness(12, 0, 0, i == listBuffer.Count - 1 ? 8 : 2);
                    target.Blocks.Add(line);
                }

                listBuffer.Clear();
            }

            foreach (var raw in lines)
            {
                var line = raw.TrimEnd();

                // 空行：结束当前列表，并留出段落间距。
                if (line.Trim().Length == 0)
                {
                    FlushList();
                    continue;
                }

                // 分隔线
                if (Regex.IsMatch(line.Trim(), @"^(-{3,}|\*{3,}|_{3,})$"))
                {
                    FlushList();

                    // Paragraph 是 Block，Run / LineBreak 是 Inline ——
                    // 加进 Blocks 的必须是前者，所以要包一层。
                    var rule = new Paragraph();
                    rule.Inlines.Add(new Run { Text = "──────────" });
                    rule.Margin = new Thickness(0, 4, 0, 8);
                    target.Blocks.Add(rule);
                    continue;
                }

                // 标题
                var heading = Regex.Match(line, @"^(#{1,3})\s+(.*)$");
                if (heading.Success)
                {
                    FlushList();
                    var level = heading.Groups[1].Value.Length;
                    var para = new Paragraph();

                    // 用字号与字重表达层级，不靠颜色 —— 颜色要与主题配合，
                    // 字号是绝对的。
                    var run = new Run { FontWeight = FontWeights.SemiBold };
                    if (level == 1)
                    {
                        run.FontSize = 18;
                    }
                    else if (level == 2)
                    {
                        run.FontSize = 16;
                    }
                    else
                    {
                        run.FontSize = 14;
                    }

                    para.Inlines.Add(run);
                    AppendInline(para, heading.Groups[2].Value, run);
                    para.Margin = new Thickness(0, 6, 0, 6);
                    target.Blocks.Add(para);
                    continue;
                }

                // 引用
                var quote = Regex.Match(line, @"^>\s?(.*)$");
                if (quote.Success)
                {
                    FlushList();
                    var para = new Paragraph();
                    para.Inlines.Add(new Run { Text = "▏ " });
                    AppendInline(para, quote.Groups[1].Value);
                    para.Margin = new Thickness(12, 2, 0, 6);
                    target.Blocks.Add(para);
                    continue;
                }

                // 无序列表
                var bullet = Regex.Match(line, @"^\s*[-*+]\s+(.*)$");
                if (bullet.Success)
                {
                    if (listOrdered)
                    {
                        FlushList();
                    }

                    listOrdered = false;
                    listBuffer.Add(bullet.Groups[1].Value);
                    continue;
                }

                // 有序列表
                var numbered = Regex.Match(line, @"^\s*\d+[.)]\s+(.*)$");
                if (numbered.Success)
                {
                    if (!listOrdered && listBuffer.Count > 0)
                    {
                        FlushList();
                    }

                    listOrdered = true;
                    listBuffer.Add(numbered.Groups[1].Value);
                    continue;
                }

                // 普通段落
                FlushList();
                var paragraph = new Paragraph();
                AppendInline(paragraph, line);
                paragraph.Margin = new Thickness(0, 0, 0, 6);
                target.Blocks.Add(paragraph);
            }

            FlushList();
        }

        // ── 行内 ────────────────────────────────────────────────────────

        /// <summary>
        /// 行内标记的正则。顺序即优先级：
        ///
        ///   1. 行内代码最先 —— 代码里出现的 `*` 与 `_` 必须当字面量，不能再去匹配强调
        ///   2. 链接
        ///   3. 粗体在斜体之前 —— `**x**` 要先于 `*x*` 匹配，否则会被拆成两个斜体
        /// </summary>
        static readonly Regex InlinePattern = new Regex(
            @"(?<code>`[^`]+`)|(?<link>\[[^\]]+\]\([^)]+\))|(?<bold>\*\*[^*]+\*\*)|(?<italic>\*[^*]+\*)",
            RegexOptions.Compiled);

        static void AppendInline(Paragraph paragraph, string text, Run template = null)
        {
            var index = 0;

            foreach (Match m in InlinePattern.Matches(text))
            {
                if (m.Index > index)
                {
                    paragraph.Inlines.Add(MakeRun(text.Substring(index, m.Index - index), template));
                }

                if (m.Groups["code"].Success)
                {
                    // 去掉两侧反引号
                    var code = m.Value.Substring(1, m.Value.Length - 2);
                    var run = new Run
                    {
                        Text = code,
                        FontFamily = new FontFamily(MonoFont),
                    };

                    if (template?.FontSize > 0)
                    {
                        // 等宽字体在同字号下视觉偏大，缩一点更协调。
                        run.FontSize = template.FontSize - 1;
                    }

                    paragraph.Inlines.Add(run);
                }
                else if (m.Groups["link"].Success)
                {
                    var link = Regex.Match(m.Value, @"^\[([^\]]+)\]\(([^)]+)\)$");
                    if (link.Success)
                    {
                        var hyperlink = new Hyperlink { NavigateUri = SafeUri(link.Groups[2].Value) };
                        hyperlink.Inlines.Add(MakeRun(link.Groups[1].Value, template));
                        paragraph.Inlines.Add(hyperlink);
                    }
                    else
                    {
                        // 解析不动就原样显示，别吞掉。
                        paragraph.Inlines.Add(MakeRun(m.Value, template));
                    }
                }
                else if (m.Groups["bold"].Success)
                {
                    var inner = m.Value.Substring(2, m.Value.Length - 4);
                    var run = MakeRun(inner, template);
                    run.FontWeight = FontWeights.SemiBold;
                    paragraph.Inlines.Add(run);
                }
                else if (m.Groups["italic"].Success)
                {
                    var inner = m.Value.Substring(1, m.Value.Length - 2);
                    var run = MakeRun(inner, template);
                    run.FontStyle = FontStyle.Italic;
                    paragraph.Inlines.Add(run);
                }

                index = m.Index + m.Length;
            }

            if (index < text.Length)
            {
                paragraph.Inlines.Add(MakeRun(text.Substring(index), template));
            }
        }

        /// <summary>
        /// 复制模板 Run 的排版属性。
        ///
        /// 标题要带字号与字重进到每个行内片段里 —— 否则标题里的粗体片段会退回
        /// 默认字号，看起来像排版错乱。
        /// </summary>
        static Run MakeRun(string text, Run template)
        {
            var run = new Run { Text = text };

            if (template == null)
            {
                return run;
            }

            if (template.FontSize > 0)
            {
                run.FontSize = template.FontSize;
            }

            if (template.FontWeight.Weight != FontWeights.Normal.Weight)
            {
                run.FontWeight = template.FontWeight;
            }

            return run;
        }

        /// <summary>
        /// 模型给的链接未必是合法 URI，而 <see cref="Hyperlink.NavigateUri"/> 遇到
        /// 非法值会抛。解析不了就返回 null —— 链接仍然可见可选中，只是点不动，
        /// 比整个页面渲染失败好。
        /// </summary>
        static Uri SafeUri(string raw)
        {
            var value = raw.Trim();

            // 相对路径与裸域名在对话里很常见，补一个 scheme 让它们可用。
            if (!value.Contains("://") && !value.StartsWith("mailto:", StringComparison.OrdinalIgnoreCase))
            {
                value = "https://" + value.TrimStart('/');
            }

            return Uri.TryCreate(value, UriKind.Absolute, out var uri) ? uri : null;
        }
    }
}
