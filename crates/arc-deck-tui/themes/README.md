# 内置主题（OpenCode v2.0.27）

这里的 JSON **逐字**取自 OpenCode tag `v2.0.27` 的 `packages/tui/src/theme/assets/`：

```
git -C <REFS>/opencode show v2.0.27:packages/tui/src/theme/assets/<name>.json
```

| 目录 | 上游路径 | 是什么 | 谁在用 |
|---|---|---|---|
| `*.json` | `packages/tui/src/theme/assets/*.json` | **v1 扁平调色板**（`defs` + `{dark, light}` 取值，没有状态角色） | 经 `src/theme/v1.rs` 的 v1→v2 迁移后使用，行为对齐上游 `packages/theme/src/tui/v1-migrate.ts` |
| `v2/opencode.json` | `packages/tui/src/theme/assets/v2/opencode.json` | **v2 语义 token 文档**（`base` + `light`/`dark`，含 `hue` 标尺与 `@dialog`） | 直接解析；同时是 Arc Deck 的默认主题 `opencode` |

上游对这两类的分流方式与这里一致：`packages/tui/src/theme/index.ts` 在看到文档里有 `theme` 键时调
`migrateV1`，否则按 v2 文档解析。`assets/v2/` 之外没有第二个 v2 文件，所以除 `opencode` 外的每个名字
都要过一次迁移。

## 许可

这些主题按上游 **MIT** 使用，Copyright (c) 2025 opencode；完整 notice 见 `crates/arc-deck-tui/LICENSE-OpenCode`。
Arc Deck 仓库本身以 Apache-2.0 发布（根 `LICENSE`），但这不改变这些资产的上游 MIT 许可与署名要求。

## 加新主题

把 `<name>.json` 放进本目录，然后在 `src/theme/assets.rs` 的 `THEMES` 里加一行
（v1 用 `v1_theme!("<name>")`）。`every_usable_asset_file_is_registered` 测试会盯着这两处
不要漏掉一处，`every_built_in_theme_resolves_in_both_modes` 会盯着每个主题在深/浅两种模式下
都能解析完 14 个 markdown token 与 9 个 syntax token。

## 移植与上游的对照

`src/theme.rs` 的 `the_v1_opencode_palette_migrates_to_upstreams_checked_in_v2_tokens` 把
`opencode.json` 的迁移结果逐 token 与 `v2/opencode.json`（上游自己检入的迁移产物）比：
两模式各自的全部 resolved token（每个状态角色、formfield、feedback、diff、markdown、syntax）
与全部 hue 标尺（10 个色名 × 9 步）**逐字相同**，唯一差异是 `categorical`
（上游那个资产早于 `categoricalTokens` 加入 `info` 的那次提交，参
`categorical_is_the_one_known_divergence`）。迁移逻辑本身另有一套纯 JS 的独立复现，
见 `tools/upstream-theme-v1-check/`。

两处**有意偏离**上游，都写在代码注释里：

1. `categorical` 按模式各自保存，而不是只存浅色那一份。上游 `base: base(lightDefinition)`
   让两种模式共用浅色的推断结果，而色名要在**本模式**的 hue 表里查——`carbonfox` 的深色
   调色板推断不出 `cyan`，上游会因此整个主题报错（`Categorical hue "cyan" was not found`）。
   参 `v1.rs` 的 `full_mode`。
2. `ThemeDialog` 的过滤是大小写不敏感的子串匹配，上游 `DialogSelect` 用 `fuzzysort`
   （打分、重排、带阈值）。参 `theme_dialog.rs` 的 `matches`。

## 没做的事

- **`system` 主题不做。** 上游的 `packages/tui/src/theme/system.ts` 读终端调色板（需要向终端发
  OSC 查询并回读），本任务范围里没有终端查询能力，所以没有 `system` 这个名字。
- **没有 16/256 色降级。** 上游 `packages/tui/src/theme/color.ts` 的 `ansiToRgba` 只把
  v1 主题里的**数字**颜色字面量转成 RGB，没有任何「按终端色彩能力把 RGB 降到 256 色」的路径；
  这里同样保持 RGB 直出（`Rgb::to_ratatui` 一律给 `Color::Rgb`）。实测过的范围仅限
  tag `v2.0.27` 里的 `packages/tui/src/theme/` 与 `packages/theme/src/`——**渲染器
  `@opentui/core` 是仓外的 npm 依赖，它在不在终端侧做降级没有查**，本客户端不用它。
