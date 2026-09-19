using System;
using System.Collections.ObjectModel;
using System.Linq;
using Windows.UI.Xaml;
using Windows.UI.Xaml.Controls;
using Windows.UI.Xaml.Navigation;

namespace ArcDeck
{
    /// <summary>工具页。左清单右详情。</summary>
    public sealed partial class ToolsPage : Page
    {
        private readonly ObservableCollection<ToolInfo> _view = new ObservableCollection<ToolInfo>();

        public ToolsPage()
        {
            InitializeComponent();
            ToolList.ItemsSource = _view;
            Refresh(string.Empty);
        }

        protected override void OnNavigatedTo(NavigationEventArgs e)
        {
            base.OnNavigatedTo(e);
            Refresh(ToolSearch.Text);
        }

        private void OnSearchTextChanged(AutoSuggestBox sender, AutoSuggestBoxTextChangedEventArgs args)
        {
            // 只在用户输入时响应。程序性改 Text 也会触发这个事件，
            // 不过滤掉的话会和 Refresh 里的选中恢复相互打架。
            if (args.Reason != AutoSuggestionBoxTextChangeReason.UserInput)
            {
                return;
            }

            Refresh(sender.Text);
        }

        private void Refresh(string keyword)
        {
            var previous = ToolList.SelectedItem as ToolInfo;

            _view.Clear();

            var matched = string.IsNullOrWhiteSpace(keyword)
                ? Store.Current.Tools
                : Store.Current.Tools.Where(t =>
                    (t.Name?.IndexOf(keyword, StringComparison.OrdinalIgnoreCase) >= 0)
                    || (t.Summary?.IndexOf(keyword, StringComparison.OrdinalIgnoreCase) >= 0)
                    || (t.Origin?.IndexOf(keyword, StringComparison.OrdinalIgnoreCase) >= 0));

            foreach (var t in matched)
            {
                _view.Add(t);
            }

            var approvalCount = Store.Current.Tools.Count(t => t.RequiresApproval);
            CountText.Text = string.IsNullOrWhiteSpace(keyword)
                ? $"{Store.Current.Tools.Count} 个可用工具 · {approvalCount} 个需要确认"
                : $"匹配 {_view.Count} / {Store.Current.Tools.Count} 个工具";

            // 筛选后尽量保住原来选中的那个，否则每次敲字详情区都会跳回占位提示。
            var keep = previous != null && _view.Contains(previous) ? previous : null;
            ToolList.SelectedItem = keep ?? _view.FirstOrDefault();
        }

        private void OnToolSelectionChanged(object sender, SelectionChangedEventArgs e)
        {
            var tool = ToolList.SelectedItem as ToolInfo;

            if (tool == null)
            {
                DetailContent.Visibility = Visibility.Collapsed;
                DetailPlaceholder.Visibility = Visibility.Visible;
                DetailPlaceholder.Text = _view.Count == 0
                    ? "没有匹配的工具。"
                    : "从左侧选一个工具看详情。";
                return;
            }

            DetailPlaceholder.Visibility = Visibility.Collapsed;
            DetailContent.Visibility = Visibility.Visible;

            DetailName.Text = tool.Name;
            DetailSummary.Text = tool.Summary;
            DetailSignature.Text = string.IsNullOrEmpty(tool.Signature) ? "（无参数）" : tool.Signature;
            DetailOrigin.Text = tool.Origin;

            // 审批那行说的是策略，不是布尔值 —— 用户要知道的是「会不会被打断」，
            // 而不是 true/false。
            DetailApproval.Text = tool.RequiresApproval
                ? "执行前会要求确认 —— 这类调用会打断你，属于有副作用的操作。"
                : "直接执行，不打断。";

            // 说明段落动态生成。数量随工具而变，写死在 XAML 里就得为每个工具
            // 备一份模板。
            //
            // 样式从 Application.Current.Resources 取而不是 Resources：后者只有
            // 本页显式声明的资源，CaptionTextBlockStyle 是框架级的，取不到会返回
            // null，赋值给 Style 会静默丢掉样式而不是报错。
            var captionStyle = Application.Current.Resources["CaptionTextBlockStyle"] as Style;

            DetailNotes.Children.Clear();
            foreach (var note in tool.Details)
            {
                var block = new TextBlock
                {
                    Text = note,
                    TextWrapping = TextWrapping.Wrap,
                    Foreground = (Windows.UI.Xaml.Media.Brush)
                        Application.Current.Resources["TextFillColorSecondaryBrush"],
                };

                if (captionStyle != null)
                {
                    block.Style = captionStyle;
                }

                DetailNotes.Children.Add(block);
            }
        }
    }
}
