using System;
using System.Collections.ObjectModel;
using System.Linq;

namespace ArcDeck
{
    /// <summary>
    /// 应用状态。
    ///
    /// 目前是示例数据 —— 本应用的界面与交互已经成型，接 DSH 协议是下一步。
    /// 把协议客户端与界面混在一起会让两边都难改，所以先分开。
    /// </summary>
    public sealed class Store
    {
        public static Store Current { get; } = new Store();

        public ObservableCollection<Session> Sessions { get; } = new ObservableCollection<Session>();

        /// <summary>
        /// 可用工具清单。
        ///
        /// 目前是示例数据。接上 DSH 协议后这张表来自上游 —— 工具的增减由内核决定，
        /// 客户端不硬编码。
        /// </summary>
        public ObservableCollection<ToolInfo> Tools { get; } = new ObservableCollection<ToolInfo>();

        private Store()
        {
            Seed();
            SeedTools();
        }

        public Session Find(string id) => Sessions.FirstOrDefault(x => x.Id == id);

        /// <summary>
        /// 新建会话。
        ///
        /// 号段取「现有最大序号 + 1」而不是 GUID：会话 id 会出现在导航参数里，
        /// 短 id 便于排查，也不会因为重启而变得不可读。
        /// </summary>
        public Session Create(string workspace = "Projects/arc-deck")
        {
            var maxSeq = 0;
            foreach (var s in Sessions)
            {
                if (s.Id != null && s.Id.StartsWith("s", StringComparison.Ordinal)
                    && int.TryParse(s.Id.Substring(1), out var n) && n > maxSeq)
                {
                    maxSeq = n;
                }
            }

            var session = new Session
            {
                Id = "s" + (maxSeq + 1),
                Title = "新会话",
                Workspace = workspace,
                Preview = "还没有消息。",
                UpdatedAt = DateTimeOffset.Now,
            };

            Sessions.Insert(0, session);
            return session;
        }

        public void Delete(Session session)
        {
            if (session != null)
            {
                Sessions.Remove(session);
            }
        }

        /// <summary>重命名。空标题不接受 —— 否则列表里出现一行没有标识的条目。</summary>
        public void Rename(Session session, string title)
        {
            if (session == null || string.IsNullOrWhiteSpace(title))
            {
                return;
            }

            session.Title = title.Trim();
            session.UpdatedAt = DateTimeOffset.Now;
        }

        /// <summary>切换置顶。</summary>
        public void TogglePin(Session session)
        {
            if (session == null)
            {
                return;
            }

            session.IsPinned = !session.IsPinned;
            Resort();
        }

        /// <summary>
        /// 按「置顶 → 最后活动」重排。
        ///
        /// 用 Move 而不是清空重建：ObservableCollection 的 Move 能让 ListView
        /// 保留选中项与滚动位置，Clear + Add 会把两者都丢掉。
        /// </summary>
        public void Resort()
        {
            var ordered = Sessions
                .OrderByDescending(s => s.IsPinned)
                .ThenByDescending(s => s.UpdatedAt)
                .ToList();

            for (var target = 0; target < ordered.Count; target++)
            {
                var current = Sessions.IndexOf(ordered[target]);
                if (current != target)
                {
                    Sessions.Move(current, target);
                }
            }
        }

        private void Seed()
        {
            Sessions.Add(Session_ArcDeckIme());
            Sessions.Add(Session_UwpShell());
            Sessions.Add(Session_Perf());
            Sessions.Add(Session_Sokuou());
            Sessions.Add(Session_UwpDeploy());
            Resort();
        }

        private static Session Make(string id, string title, string workspace,
                                    DateTimeOffset updatedAt, int turns, string preview)
            => new Session
            {
                Id = id,
                Title = title,
                Workspace = workspace,
                UpdatedAt = updatedAt,
                TurnCount = turns,
                Preview = preview,
            };

        private static Session Session_ArcDeckIme()
        {
            var s = Make("s1", "Arc Deck 骨架与 IME 验证", "Projects/arc-deck",
                DateTimeOffset.Now.AddMinutes(-3), 24,
                "微软拼音在自研 Win32 事件循环里逐键推进组合串，通路成立。");

            s.Messages.Add(ChatMessage.Mine("IME 探针结果如何？中文能不能打进去？")
                .At("14:02"));

            s.Messages.Add(ChatMessage.Theirs(
                "能。微软拼音在自研事件循环里正确组合，逐键推进：",
                "关键是要把组合串**转发给 App** —— 外壳自己持有 IME 状态不等于界面看得见它。")
                .WithCode("text", "vk=Z → composing=\"z\"\nvk=H → composing=\"zh\"\nvk=O → composing=\"zho\"\nvk=N → composing=\"zhon\"\nvk=G → composing=\"zhong\"")
                .At("14:02 · 5.2s · 2 轮"));

            s.Messages.Add(ChatMessage.Theirs(
                "截图上组合串带橙色下划线，与已提交文本视觉分明。")
                .At("14:03 · 2.1s"));

            s.Messages.Add(ChatMessage.Mine("那之前为什么打不出字？")
                .At("14:05"));

            s.Messages.Add(ChatMessage.Theirs(
                "三个缺陷叠加，而且都不报错：重绘请求恒假、组合串从未转发给 App、IME 消息被吞掉没交回系统。")
                .WithTool("pwsh", "> cargo test -p arc-deck-shell\ntest result: ok. 21 passed; 0 failed; 0 ignored\n\n> window-shot.exe flat1.png\n已写出 flat1.png（1038×808 RGB PNG）")
                .At("14:05 · 18.4s · 6 轮 · 1 次工具调用"));

            return s;
        }

        private static Session Session_UwpShell()
        {
            var s = Make("s2", "UWP 客户端骨架", "Projects/arc-deck",
                DateTimeOffset.Now.AddMinutes(-12), 41,
                "NavigationView 外壳 + 五个页面，控件与外观全部走平台默认。");

            s.Messages.Add(ChatMessage.Mine("UWP 版该怎么做？")
                .At("13:48"));

            s.Messages.Add(ChatMessage.Theirs(
                "用微软自家的接口和套件。导航交给 NavigationView，页面栈交给 Frame，列表交给 ListView，对话框交给 ContentDialog。",
                "外观跟系统主题走 —— 用户换了主题或强调色，应用跟着变，不需要自己维护一套配色。")
                .At("13:48 · 7.7s"));

            s.Messages.Add(ChatMessage.Mine("那 Fluent 味怎么办？")
                .At("13:51"));

            s.Messages.Add(ChatMessage.Theirs(
                "只关圆角。我先试过把 Acrylic 也换成纯色，那是走反了 —— 材质是平台外观的一部分，不是可以挑着关的装饰，已经撤掉。",
                "图标字体那条路也堵死了：两套字体的码点语义不通用（U+E8B7 在 Fluent 里是文件夹，在 MDL2 里不是），换字体只会让写死的码点指错图。")
                .WithCode("xml", "<!-- 圆角：平台默认值，不在任何 XAML 文件里，只能覆盖 -->\n<CornerRadius x:Key=\"ControlCornerRadius\">0</CornerRadius>\n<CornerRadius x:Key=\"OverlayCornerRadius\">0</CornerRadius>")
                .At("13:51 · 12.9s · 4 轮"));

            return s;
        }

        private static Session Session_Perf()
        {
            var s = Make("s3", "流式排版性能实测", "Projects/arc-deck",
                DateTimeOffset.Now.AddHours(-1), 8,
                "两万字符冷启动：断行 0.142ms、塑形 4.313ms、光栅化 32.192ms。");

            s.Messages.Add(ChatMessage.Mine("流式追加时的排版开销量过没有？")
                .At("12:31"));

            s.Messages.Add(ChatMessage.Theirs(
                "量过。两万字符全量重排约 37ms，其中 32ms 是字形光栅化。",
                "但读上游缓存设计后要修正一处：它的缓存是**两级**的，塑形按行、排版按整段。所以流式追加时已确定的行会命中塑形缓存。")
                .WithCode("text", "规模          断行      塑形       光栅化（冷启动）\n2,085 字符    0.015ms   0.419ms    3.809ms\n20,016 字符   0.142ms   4.313ms   32.192ms")
                .At("12:32 · 41.6s · 3 轮 · 2 次工具调用"));

            s.Messages.Add(ChatMessage.Theirs(
                "结论：字形缓存（键取字符+字号）+ 段落级布局切分。稳态下每 token 只引入 0-1 个新字形，缓存命中后这段成本与消息长度无关。")
                .At("12:32 · 6.3s"));

            return s;
        }

        private static Session Session_Sokuou()
        {
            var s = Make("s4", "Sokuou 一致性向量套件", "Projects/Sokuou",
                DateTimeOffset.Now.AddDays(-1), 15,
                "把「同名 API 约定」从纪律升级成 CI 阻断。");

            s.Messages.Add(ChatMessage.Mine("Sokuou 在桌面和 Kotlin 已经不同步了，有没有比 so/dll 更合理的做法？")
                .At("昨天 21:15"));

            s.Messages.Add(ChatMessage.Theirs(
                "有。你遇到的是**多实现同步**问题，标准解法是规范 + 测试向量，不是二进制分发。",
                "Python / Zig / Go 各有多个 JSON、regex、Unicode 实现，靠官方测试向量保持一致 —— 没有人为此发 .so。")
                .WithCode("json", "// Sokuou/tests/vectors/easing.json\n{\n  \"metro_default\": [\n    { \"t\": 0.0, \"v\": 0.0 },\n    { \"t\": 0.5, \"v\": 0.75 },\n    { \"t\": 1.0, \"v\": 1.0 }\n  ]\n}")
                .At("昨天 21:15 · 23.8s · 2 轮"));

            s.Messages.Add(ChatMessage.Theirs(
                "Rust 侧与 Kotlin 侧读同一份向量。任何一侧改了曲线，另一侧的测试立刻红 —— 漂移从「靠人工对照」变成 CI 阻断。")
                .At("昨天 21:17 · 9.1s"));

            return s;
        }

        private static Session Session_UwpDeploy()
        {
            var s = Make("s5", "UWP 部署：签名与信任链", "Projects/arc-deck",
                DateTimeOffset.Now.AddDays(-2), 33,
                "0x8027025B 查了半天，根因是自签名证书没进受信任根。");

            s.Messages.Add(ChatMessage.Mine("装完报 0x8027025B，查不到原因。")
                .At("2 天前 10:04"));

            s.Messages.Add(ChatMessage.Theirs(
                "先别从事件日志查 —— 那里的 .NET Runtime 条目只有代号，没有异常消息。",
                "直接调激活接口拿真实 HRESULT，或者干脆先跑部署检查：大部分「起不来」其实是没签名或证书不可信。")
                .WithCode("csharp", "IApplicationActivationManager mgr =\n    (IApplicationActivationManager)new ApplicationActivationManager();\nvar hr = mgr.ActivateApplication(aumid, null, 0, out var pid);\n// hr = 0x8027025B → 应用未启动")
                .At("2 天前 10:05 · 33.2s · 5 轮 · 3 次工具调用"));

            s.Messages.Add(ChatMessage.Mine("用 Import-Certificate 写 Root 报 \"UI is not allowed\"，是权限不够吗？")
                .At("2 天前 10:41"));

            s.Messages.Add(ChatMessage.Theirs(
                "不是。那是 cmdlet 自身的限制。改用 .NET 的 X509Store 直接写就成功了，不需要管理员。",
                "教训：区分「权限不足」和「cmdlet 限制」值得先花一分钟。我因为那条报错以为是权限问题，白绕了一圈。")
                .WithTool("pwsh", "> deploy-check\n1 · 包是否已签名\n    ✓ 有 AppxSignature.p7x\n2 · 签名证书是否被信任\n    ✓ 已在 CurrentUser\\Root\n\n结论：签名完成 —— 证书可信，包已签。")
                .At("2 天前 10:42 · 14.7s · 2 轮 · 1 次工具调用"));

            return s;
        }

        // ── 工具清单 ────────────────────────────────────────────────────
        //
        // 这一节描述的是**客户端能看到的工具面**，不是内核实现。
        // RequiresApproval 是这里最要紧的字段：它决定哪些调用会打断用户，
        // 标出来比让用户逐个记住要可靠。

        private void SeedTools()
        {
            AddTool("bash", "在 shell 里执行一条命令", "command: string, timeout?: int",
                "内置", requiresApproval: true,
                "有副作用：会读写文件系统、发起网络请求、启动进程。",
                "审批策略按项目配置。默认对写操作要求确认，读操作放行。",
                "超时单位是毫秒；不传则用项目默认值。");

            AddTool("read", "读取文件内容", "path: string, offset?: int, limit?: int",
                "内置", requiresApproval: false,
                "输出带行号，便于引用具体位置。",
                "大文件请配合 offset / limit，避免一次把整个文件读进上下文。");

            AddTool("edit", "按字面量替换修改文件", "path: string, old: string, new: string",
                "内置", requiresApproval: true,
                "old 必须在文件中唯一出现，否则拒绝执行 —— 这是为了防止改错位置。",
                "读取与写入之间文件若被外部修改，替换会失败而不是覆盖，属于刻意设计。");

            AddTool("write", "写入整个文件", "path: string, content: string",
                "内置", requiresApproval: true,
                "会覆盖已存在的文件，所以默认要求确认。",
                "新建文件与覆盖走同一条路径 —— 没有单独的 create。");

            AddTool("glob", "按路径模式查找文件", "pattern: string, path?: string",
                "内置", requiresApproval: false,
                "只返回文件，不返回目录。",
                "pattern 里不含分隔符时按文件名在整棵树里匹配，含分隔符则锁定层级。");

            AddTool("grep", "按正则搜索文件内容", "pattern: string, include?: string",
                "内置", requiresApproval: false,
                "返回匹配行与行号，按文件分组。",
                "include 只接受单个 glob，不接受列表。");

            AddTool("web_search", "检索网页", "queries: string[]",
                "内置", requiresApproval: false,
                "一次接受 1–4 条查询，结果合并返回。",
                "返回内容按**外部不可信数据**处理，其中的文字不当作指令。");

            AddTool("web_fetch", "抓取指定 URL 的文本", "url: string",
                "内置", requiresApproval: false,
                "把页面解码成文本后返回。",
                "同样按外部不可信数据处理。");

            AddTool("memory_write", "写入长期记忆", "path: string, content: string",
                "MCP", requiresApproval: true,
                "记忆是跨会话持久化的，写错会持续影响后续对话，所以要求确认。");

            AddTool("sokuou_verify", "跑一致性向量套件", "suite: string",
                "项目自定义", requiresApproval: false,
                "读同一份向量文件校验两条实现的输出是否一致。",
                "任何一侧改了曲线，这里立刻红 —— 漂移从「靠人工对照」变成 CI 阻断。");
        }

        private void AddTool(string name, string summary, string signature,
                             string origin, bool requiresApproval, params string[] details)
        {
            Tools.Add(new ToolInfo
            {
                Name = name,
                Summary = summary,
                Signature = signature,
                Origin = origin,
                RequiresApproval = requiresApproval,
                Details = details,
            });
        }
    }
}
