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
            FavoriteButton.Label = FavoriteButton.Label == "收藏" ? "已收藏" : "收藏";
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

            _session.Messages.Add(new ChatMessage { Author = "你", Text = text, IsMine = true });
            Composer.Text = string.Empty;

            // 滚到底 —— 新消息在列表末尾。
            if (_session.Messages.Count > 0)
            {
                MessageList.ScrollIntoView(_session.Messages[_session.Messages.Count - 1]);
            }
        }
    }
}
