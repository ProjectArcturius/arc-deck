using System;
using Windows.Storage;
using Windows.UI.Xaml;

namespace ArcDeck
{
    /// <summary>主题偏好。</summary>
    public enum AppTheme
    {
        /// <summary>跟随系统。</summary>
        System = 0,

        Light = 1,

        Dark = 2,
    }

    /// <summary>
    /// 用户偏好。存在 <see cref="ApplicationData.Current"/> 的 LocalSettings 里。
    ///
    /// 为什么需要这个：设置页原先摆了三个单选按钮，但**勾了没有任何效果** ——
    /// 主题没被应用，选项也不持久化，重开就回到「跟随系统」。那是最糟的一种
    /// 界面：看起来能配，实际不生效。
    ///
    /// 用 LocalSettings 而不是自己写文件：UWP 里它是现成的键值存储，
    /// 随应用生命周期管理，卸载时一并清理，不需要自己处理路径与并发。
    /// </summary>
    public static class Settings
    {
        const string ThemeKey = "app.theme";

        /// <summary>读取主题偏好。读不到或值非法时回落到跟随系统。</summary>
        public static AppTheme Theme
        {
            get
            {
                var raw = ApplicationData.Current.LocalSettings.Values[ThemeKey];

                if (raw is int i && Enum.IsDefined(typeof(AppTheme), i))
                {
                    return (AppTheme)i;
                }

                return AppTheme.System;
            }
            set
            {
                ApplicationData.Current.LocalSettings.Values[ThemeKey] = (int)value;
            }
        }

        /// <summary>
        /// 把主题应用到一棵已经加载的元素树。
        ///
        /// 为什么改 <c>RequestedTheme</c> 而不是 <c>Application.RequestedTheme</c>：
        /// 后者的 setter 不是公开的，只能在应用启动前由系统决定。可控的那个是
        /// <see cref="FrameworkElement.RequestedTheme"/>，它会让整棵子树
        /// 重新解析 ThemeResource。
        ///
        /// 传 System 时用 ElementTheme.Default —— 那是「跟随系统」在元素层的写法。
        /// </summary>
        public static void ApplyTheme(FrameworkElement root)
        {
            if (root == null)
            {
                return;
            }

            switch (Theme)
            {
                case AppTheme.Light:
                    root.RequestedTheme = ElementTheme.Light;
                    break;
                case AppTheme.Dark:
                    root.RequestedTheme = ElementTheme.Dark;
                    break;
                default:
                    root.RequestedTheme = ElementTheme.Default;
                    break;
            }
        }
    }
}
