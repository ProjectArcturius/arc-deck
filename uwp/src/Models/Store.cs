using System.Collections.ObjectModel;
using System.Linq;

namespace ArcDeck
{
    /// <summary>
    /// 应用状态。
    ///
    /// 这是示例数据 —— 本应用当前的目标是「一个用微软自家套件的标准 UWP 客户端」。
    /// 接 DSH 协议是后续的事，不该混在界面骨架里。
    /// </summary>
    public sealed class Store
    {
        public static Store Current { get; } = new Store();

        public ObservableCollection<Session> Sessions { get; } = new ObservableCollection<Session>();

        private Store()
        {
            Seed();
        }

        public Session Find(string id) => Sessions.FirstOrDefault(x => x.Id == id);

        private void Seed()
        {
            Add("s1", "Arc Deck 骨架与 IME 验证", "Projects/arc-deck", "刚刚", 24,
                "中文 IME 在自研事件循环里组合成功，组合串带下划线正确渲染。",
                ("你", "IME 探针结果如何？中文能不能打进去？", true),
                ("助手", "能。微软拼音逐键推进 z → zh → zho → zhon → zhong，组合串正确渲染，与已提交文本视觉分明。", false));

            Add("s2", "UWP 客户端骨架", "Projects/arc-deck", "12 分钟前", 41,
                "NavigationView 外壳 + 会话列表 + 对话页，全部用平台自带控件的默认外观。",
                ("你", "UWP 版该怎么做？", true),
                ("助手", "用微软自家的接口和套件：NavigationView 做导航、Frame 管页面栈、ListView 显示列表、ContentDialog 做对话框。外观跟系统主题走，不覆盖。", false));

            Add("s3", "流式排版性能实测", "Projects/arc-deck", "1 小时前", 8,
                "两万字符冷启动：断行 0.142ms、塑形 4.313ms、光栅化 32.192ms。",
                ("你", "流式追加时的排版开销量过没有？", true),
                ("助手", "量过。两万字符全量重排约 37ms，其中 32ms 是字形光栅化 —— 按 (字符, 字号) 做缓存即可让这段与消息长度无关。", false));

            Add("s4", "Sokuou 一致性向量套件", "Projects/Sokuou", "昨天", 15,
                "把「同名 API 约定」从纪律升级成 CI 阻断。",
                ("你", "Sokuou 在桌面和 Kotlin 已经不同步了，怎么办？", true),
                ("助手", "做成一致性测试向量。Rust 侧与 Kotlin 侧读同一份向量文件，任何一侧改了曲线，另一侧的测试立刻红。", false));

            Add("s5", "Ether launcher 迁移收尾", "Projects/Ether", "2 天前", 33,
                "文档仍在说 launcher 是 egui 形态，实际早已迁完。",
                ("你", "launcher 现在什么形态？", true),
                ("助手", "已经是 Kanesumi 形态了。Cargo.toml 里连 egui 依赖都没有，是文档陈旧。", false));
        }

        private void Add(string id, string title, string workspace, string updatedAt,
                         int turns, string preview, params (string author, string text, bool mine)[] messages)
        {
            var s = new Session
            {
                Id = id,
                Title = title,
                Workspace = workspace,
                UpdatedAt = updatedAt,
                TurnCount = turns,
                Preview = preview,
            };

            foreach (var (author, text, mine) in messages)
            {
                s.Messages.Add(new ChatMessage { Author = author, Text = text, IsMine = mine });
            }

            Sessions.Add(s);
        }
    }
}
