using System;
using Windows.UI.Xaml;
using Windows.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Controls;

namespace ArcDeck
{
    /// <summary>工具页：WinUI 2 控件一览。</summary>
    public sealed partial class ToolsPage : Page
    {
        public ToolsPage()
        {
            InitializeComponent();

            Breadcrumb.ItemsSource = new[] { "工作区", "arc-deck", "工具" };
        }

        private async void OnOpenDialog(object sender, RoutedEventArgs e)
        {
            // ContentDialog 是 UWP 的标准对话框：自动居中、带遮罩、Esc 关闭、
            // 焦点圈在对话框内。不需要自己拼浮层。
            var dialog = new ContentDialog
            {
                Title = "ContentDialog",
                Content = "平台自带的对对话框。这里也可以测试中文输入法。\n\n输入框：",
                PrimaryButtonText = "确定",
                CloseButtonText = "取消",
                DefaultButton = ContentDialogButton.Primary,
                XamlRoot = XamlRoot,
            };

            await dialog.ShowAsync();
        }

        private void OnOpenTeachingTip(object sender, RoutedEventArgs e)
        {
            var tip = new TeachingTip
            {
                Title = "TeachingTip",
                Subtitle = "WinUI 2 控件",
                Content = "用于引导用户注意某处。",
                IsOpen = true,
                PreferredPlacement = TeachingTipPlacementMode.Center,
                XamlRoot = XamlRoot,
            };

            tip.IsOpen = true;
        }
    }
}
