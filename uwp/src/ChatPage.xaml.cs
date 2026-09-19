using System;
using Windows.ApplicationModel.DataTransfer;
using Windows.System;
using Windows.UI.Xaml;
using Windows.UI.Xaml.Controls;
using Windows.UI.Xaml.Input;
using Windows.UI.Xaml.Navigation;

namespace ArcDeck
{
    /// <summary>对话页。</summary>
    public sealed partial class ChatPage : Page
    {
        private Session _session;

        /// <summary>收藏状态。存在页面里而不是会话上 —— 会话模型还没有这个字段。</summary>
        private bool _favorited;

        public ChatPage()
        {
            InitializeComponent();
        }

        protected override void OnNavigatedTo(NavigationEventArgs e)
        {
            base.OnNavigatedTo(e);

            if (e.Parameter is string id)
            {
                _session = Store.Current.Find(id);
            }

            if (_session == null)
            {
                return;
            }

            TitleText.Text = _session.Title;
            MetaText.Text = _session.Subtitle;
            MessageList.ItemsSource = _session.Messages;

            // 进入后把焦点给输入框 —— 打开会话就能直接打字。
            Composer.Focus(FocusState.Programmatic);
        }

        private void OnBackClick(object sender, RoutedEventArgs e)
        {
            if (Frame.CanGoBack)
            {
                Frame.GoBack();
            }
        }

        private void OnFavoriteClick(object sender, RoutedEventArgs e)
        {
            // 换成 Button 之后不能再改 Label（Button 没有这个属性）。
            // 收藏状态用图标本身表达：实心星 = 已收藏，空心星 = 未收藏。
            //
            // 这两个码点是同一套图标字体里的一对：E734 空心、E735 实心。
            // 它们在 Segoe MDL2 Assets 与 Segoe Fluent Icons 里都存在且含义相同，
            // 所以切换是安全的 —— 换成别的字形就要重新核对，两套字体并非码点通用。
            _favorited = !_favorited;

            FavoriteIcon.Glyph = _favorited ? "\uE735" : "\uE734";
            ToolTipService.SetToolTip(FavoriteButton, _favorited ? "取消收藏" : "收藏");
        }

        private void OnCopyPathClick(object sender, RoutedEventArgs e)
        {
            if (_session == null)
            {
                return;
            }

            var package = new DataPackage();
            package.SetText(_session.Workspace);
            Clipboard.SetContent(package);
        }

        private void OnReloadClick(object sender, RoutedEventArgs e)
        {
            // 目前只是回到列表再进来 —— 重载真实会话要等协议客户端接上。
            if (Frame.CanGoBack)
            {
                Frame.GoBack();
            }
        }

        private void OnComposerKeyDown(object sender, KeyRoutedEventArgs e)
        {
            if (e.Key == VirtualKey.Enter)
            {
                e.Handled = true;
                Send();
            }
        }

        private void OnSendClick(object sender, RoutedEventArgs e) => Send();

        private void Send()
        {
            var text = Composer.Text;
            if (string.IsNullOrWhiteSpace(text) || _session == null)
            {
                return;
            }

            _session.Messages.Add(ChatMessage.Mine(text)
                .At(DateTime.Now.ToString("HH:mm")));
            Composer.Text = string.Empty;

            // 滚到底 —— 新消息在列表末尾。
            if (_session.Messages.Count > 0)
            {
                MessageList.ScrollIntoView(_session.Messages[_session.Messages.Count - 1]);
            }
        }
    }
}
