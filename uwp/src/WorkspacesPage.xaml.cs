using System.Collections.ObjectModel;
using System.Linq;
using Windows.UI.Xaml.Controls;

namespace ArcDeck
{
    /// <summary>工作区页：把会话按工作区归类。</summary>
    public sealed partial class WorkspacesPage : Page
    {
        public sealed class WorkspaceRow
        {
            public string Path { get; set; }
            public string Summary { get; set; }
            public int Count { get; set; }
        }

        public WorkspacesPage()
        {
            InitializeComponent();

            var rows = Store.Current.Sessions
                .GroupBy(s => s.Workspace)
                .Select(g => new WorkspaceRow
                {
                    Path = g.Key,
                    Count = g.Count(),
                    Summary = string.Join("、", g.Select(s => s.Title).Take(2))
                              + (g.Count() > 2 ? " 等" : string.Empty),
                })
                .OrderByDescending(r => r.Count)
                .ToList();

            WorkspaceList.ItemsSource = new ObservableCollection<WorkspaceRow>(rows);
        }

        private void OnWorkspaceClick(object sender, ItemClickEventArgs e)
        {
            // 点工作区 → 回到会话页（后续可扩展为带筛选参数导航）
            if (Frame.CanGoBack)
            {
                Frame.GoBack();
            }
        }
    }
}
