# Arc Deck · 设计系统

> 正典：[Kanesumi Design](https://github.com/ProjectArcturius/Ether/blob/main/docs/KANESUMI_DESIGN.md)
> （`Ether/docs/KANESUMI_DESIGN.md`，108 行）。
> 本文只记录**落地取值**与**已核实的落差** —— 语言本体以正典为准，不在此复述。

## 四根支柱（落地方式）

| 支柱 | 在代码里的强制方式 |
|---|---|
| **直角** | `SceneCommand` 的 `FillRect`/`StrokeRect` **没有 radius 字段** |
| **贴边** | 无 topBar 槽；顶栏是内容的第一项 |
| **深底单强调色** | `SceneCommand` **没有渐变变体** —— 结构性不可能 |
| **受控混合** | 弹簧只用于主运动；次属性走 UWP 缓动 |

**「无渐变」不是纪律，是命令模型的形状。** 上游 Kanesumi 的
`SceneCommand` 同样只有八个变体，无画笔/渐变。这一点上游做过复查：
Android 侧 21 个 `.kt` 文件里 `RoundedCornerShape`/`clip(`/`shadow`/
`elevation` 匹配 12 次，**全部在 KDoc 注释里，代码里零次**。

## 几何

| 项 | 值 | 来源 |
|---|---|---|
| 圆角 | **0.0（默认）** | `CornerRadius::Square` |
| 圆角例外 1 | `Slight` = 2.0 | 逐例 opt-in |
| 圆角例外 2 | `Capsule` = `min(w,h)/2` | **仅** Switch 轨道/滑块、进度条 |
| 描边 | 1px，**向内** | 与「贴边」一致：不侵占相邻元素像素 |
| 阴影 | **无** | 纵深靠明暗，不靠阴影 |
| 渐变 | **无** | 命令模型无此变体 |

上游实测：172 处 `CornerRadius`，**只有 5 处非零**，全是
`CornerRadius::same(3)`，且集中在 PDF 缩略图 —— 那是不一致，不是系统。
PezMax-One 同样：零 `Shadow`、零渐变、圆角近乎全零。

## 色彩 —— ⚠ 必须显式记录的平台分歧

正典写「OLED 纯黑 `#000000`」，**Android 扇区严格遵守，Ether 桌面扇区不遵守**：

| Token | **Ether 桌面（Arc Deck 跟随）** | Android |
|---|---|---|
| background | **`#1A1A1A`** | `#000000` |
| surface | `#242424` | `#0A0F16` |
| surface_variant | `#2E2E2E` | `#14181F` |
| divider | `#3A3A3A` | `#2A2A2A` |
| **primary** | **`#E57812`（橙金）** | `#2E67B5`（蓝） |
| on_background / on_surface | `#F0F0F0` | `#F0F0F0` |
| on_surface_variant | `#9AA0A6` | `#9AA0A6` |
| press_tint | `#FFFFFF1A`（白 10%） | `#22FFFFFF`（白 13.3%） |
| focus_stroke | `#FFA626` | — |

**分歧没有文档解释。** 两个平台都满足「深底 + 灰阶分层 + 单一强调色」，
但都不是字面正典。Arc Deck 选择**跟随桌面扇区**，并把这条写进文档 ——
因为一个未记录的取值分歧，半年后会变成"为什么这里跟文档不一样"的困惑。

### 调色板是一个标量的纯函数

正典色彩落地的最佳形态（PezMax 验证过）：每个 token 是
`fn token() -> Color`，通过 `lerp_dark(light, dark)` 读取单个 0→1 的
`dark_progress()` 标量。**于是整套主题交叉淡入淡出只花一个 float。**

Arc Deck 的 `Color::lerp` 就是为此准备的。

## 排版

上游 Rust 与 Android **在这块数值完全一致**（罕见的一致点）：

**语义轴**（偏好用于新代码 —— 表达"这段文字在页面上是什么角色"）：

| Token | size | line-height | 比值 |
|---|---|---|---|
| `page_heading` | 34.0 | 42.0 | 1.235 |
| `title` | 22.0 | 28.0 | 1.273 |
| `body` | 15.0 | 22.0 | 1.467 |
| `caption` | 13.0 | 18.0 | 1.385 |
| `label` | 11.0 | 14.0 | 1.273 |

**尺度轴**（M3 命名，**取值是 Kanesumi 自己的**）：`headline_medium` 28/36、
`title_large` 22/28、`title_medium` 16/24 Medium、`body_large` 16/24、
`body_medium` 14/20、`body_small` 12/16。

- **CJK 行距必须比默认更紧** —— 上游有断言 `body.line_height / body.size < 1.5`
- 字体族：思源黑体（Source Han Sans SC）。**加载失败必须硬失败**，不得静默回退
  （正典 SD §IX）—— 否则中文字形会无声变成豆腐块
- 字号一律**逻辑像素**

## 运动

**主运动**（位移 / 缩放 / 面板滑入）→ 解析解弹簧。
**次属性**（透明度 / 颜色 / 非物理位移）→ 时长驱动 + UWP 缓动。**次属性绝无弹簧回弹。**

### 弹簧预设（上游 Rust 侧，权威）

| 预设 | response | damping_ratio |
|---|---|---|
| `standard_interaction` | 0.50 | 0.825 |
| `quick_interaction` | 0.30 | 0.60 |
| `slow_reveal` | 0.65 | 0.85 |
| `dialog_enter` | 0.45 | 0.70 |
| `page_transition` | 0.40 | 0.80 |

求解器是**解析闭式**（非欧拉积分）：帧率无关、无条件稳定、可中断。
`SNAP_THRESHOLD = 0.001`（位置与速度同时低于才吸附）；
`STALL_SNAP_THRESHOLD = 0.25` 秒 —— **`dt` 超过 0.25s 判定为系统停滞，直接吸附**，
避免窗口拖拽后出现可见的「慢放尾巴」（PezMax 实测记录过这个 bug）。

> PezMax 的实测经验：实际在用的标准是 `0.4 / 0.8`（用了 10 次），
> 而它自己的文档推荐的 `0.3/0.6`、`0.65/0.85`、`0.45/0.7` **零调用点**。
> **文档会漂移，代码是权威。**

### 时长（Rust 侧是权威 —— Android 落后）

| 常量 | 正确值 | Android 落后值 |
|---|---|---|
| `METRO_STANDARD_DURATION` | **0.25s** | — |
| `QUICK_SWITCH` | **0.167s** | 180ms |
| `TOGGLE_FLIP` | **0.15s** | 220ms |

默认曲线 `Quadratic / EaseOut`（`metro_default(0.5) == 0.75`）。

**退出永远短于进入** —— 反复出现的刻意不对称：sheet 0.30/0.26、
overlay 0.383/0.216、dialog fade 0.167/0.083、expander 0.333/0.167。

## ⚠ 已核实的落差（照抄会踩坑）

1. **上游 Rust 的 `MetroIndication` 退化了。** Android 是动画的（按下 100ms
   EaseOutCubic 进 / 松开 **200ms** 出，单个 `Animatable` 在 draw phase 读，
   一次性 `drawRect` **无圆角参数** —— 用 API 形状强制直角）。但 Rust 侧变成了
   **静态 token 结构体**，28 处用法**无一动画，全是硬切**。
   正典说「直角**闪切**」—— **这里是 Arc Deck 要补的。**

2. **无障碍在 Rust 侧完全不存在。** `Scene` 是纯绘制命令，**没有无障碍树**。
   正典铁律 7「语义树自层 0 起」**只在 Android 实现**。
   **这是全项目最大的缺口。**

3. **浅色主题是「正典有、实现无」** —— 正典说是二态之一，Rust 只有
   `ether_dark()`，Android 只有一个调色板。

4. `MetroSwitch` 几何分歧：Android 52×28dp vs 上游 `CONTROL_SPEC` 的
   UWP 原版 40×20。

## 从 PezMax-One 吸收的经验（已核实）

PezMax-One 是同一作者的**更早**的 Rust 桌面作品，用 egui，叫「Metro Design」。
它**不是** Kanesumi 实现（全仓 `Kanesumi` 一次未出现），但它的设计产出被认可，
因此以下经验值得吸收：

### 值得直接采纳

| 经验 | 说明 |
|---|---|
| **调色板 = 一个标量的纯函数** | 整套主题交叉淡入淡出只花一个 float |
| **中心化动画推进 + 中心化重绘门控** | 一处判断「有没有东西在动」；实测修掉了「一个 GIF 头像就让空闲时全屏 60fps 空转」 |
| **`render(...) -> Intent` 纯函数** | 组件返回小枚举/元组，一个 orchestrator 派发 |
| **3px `primary()` 强调条** | 悬浮表面的前缘签名图案 |
| **中断安全的交叉淡入** | 重触发时读取**当前插值**作为新的起点 —— 中途切换强调色不会跳变 |
| **`STALL_SNAP_THRESHOLD` 的 4Hz 论证** | 60/30/10/4 Hz 逐级推导，修掉 Windows 拖拽慢放尾巴 |
| **面板即「矩形 + 填充 + 裁剪」** | 它把所有面板的 `show_separator_line(false)` + `Margin::ZERO` + `Stroke::NONE`，于是多面板外壳在视觉上是无边框的 |
| **显式命名的矩形，而非测量** | 对话框把 `max_rect()` 切成具名 `Rect`，不靠游标测量 |

### 必须反着做

| 问题 | PezMax 的做法 | Arc Deck |
|---|---|---|
| **emoji 当图标** | ~15 处 `☰🔍🔔📥🚩★` | **禁止** —— `Scene` 渲染器画不出彩色 emoji，且破坏单色语言 |
| **无间距 token** | 66 处 `Frame` + 100+ 处 `add_space` 全是字面量，`24.0` 在单文件重复十次 | 建立间距 token |
| **声明式刻度被忽略** | 定义了 8 级字号，实际用 11/15/18/22/36/42 等**离刻度**值 | 刻度即约束，调用点不得散写字面量 |
| **硬编码色绕过 token** | ~20 处十六进制字面量，含一个**第六个背景色** `#1E1E2E` | 全部走 token；主题过渡才不会有色块闪现 |
| **对话框无遮罩** | 6 个对话框浮在未变暗的活内容上，`overlay()` token 定义了却**从未调用** | 显式决策，不继承 |
| **对比度不足** | 侧栏静音文字 ≈3.1:1（AA 不达标）；白字配 `#00BC70` ≈2.1:1 | 上线前审计 |
| **「一切动画走 Sokuou」被自身违反** | 进度条用裸 `i.time` 正弦；toast 寿命用 `Instant` 墙钟 | 遵守自己的规则，或改规则 |

### 三个真实的工期风险（PezMax 里完全不可见，因为 egui 全包了）

1. **文本编辑** —— 光标 / 选区 / IME / 剪贴板
2. **滚动** —— 偏移 + 裁剪 + 滚轮
3. **命中测试 / 交互** —— `Sense` / `Response` / hover / click / pointer-down

**这三项不在任何现存代码里，必须自己写。** 它们才是真正的风险所在，
而不是控件数量。
