//! Model catalog behind the model dialog: data, local preferences and ordering only.
//! The provider list is handed in by the caller (its authoritative source is DSH, §7.2),
//! so nothing here touches the kernel, the config file or the terminal.
//!
//! Ported from OpenCode v1.19.0 (MIT, notice: ../LICENSE-OpenCode):
//! `packages/tui/src/util/model.ts`（路由切分与展示名回退）、
//! `packages/tui/src/component/dialog-model.tsx`（Favorites/Recent 分节与分组排序）、
//! `packages/tui/src/context/local.tsx`（recent 去重上限 10、收藏切换、按模型存强度）。
//!
//! 三处与上游的有意差异，理由写在各自的注释里：
//! 1. `category` 放 provider 名（上游放分节名），所以 `description` 不重复 provider 名；
//! 2. 搜索分支按模糊分数排序（上游在 fuzzysort 之后又按 release_date 重排一次，分数被丢掉）；
//! 3. provider 之间只按 name 排序，不给某个 provider 提前（Arc Deck 的目录来自 DSH，不是内置的）。

use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// 最近使用只留 10 条，与上游 `recentModels` 的 `.slice(0, 10)` 一致。
pub const RECENT_LIMIT: usize = 10;

/// 一条路由 `provider_id/model_id`。
///
/// 字段是 snake_case 而不是上游状态文件里的 `providerID`/`modelID`：这份偏好属于
/// Arc Deck 自己的状态目录，不是 OpenCode 的 `model.json`，没有兼容负担。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ModelRef {
    pub provider_id: String,
    pub model_id: String,
}

impl ModelRef {
    /// `"provider/model"` —— 写回路由、也给 effort 表当键，只此一处拼法。
    pub fn route(&self) -> String {
        format!("{}/{}", self.provider_id, self.model_id)
    }
}

/// 按**第一个** `/` 切开，其余全部归 model_id：`a/b/c` → provider `a`、model `b/c`。
/// 上游 `util/model.ts` 就是这么写的（`const [providerID, ...modelID] = value.split("/")`），
/// model_id 里带 `/` 的路由才能用。provider 或 model 为空返回 `None`。
pub fn parse_route(route: &str) -> Option<ModelRef> {
    let (provider_id, model_id) = route.split_once('/')?;
    if provider_id.is_empty() || model_id.is_empty() {
        return None;
    }
    Some(ModelRef {
        provider_id: provider_id.to_string(),
        model_id: model_id.to_string(),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provider {
    pub id: String,
    pub name: String,
    pub models: Vec<Model>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Model {
    pub id: String,
    pub name: Option<String>,
    /// ISO 日期字符串，按字典序就是时间序；缺数据时排序要能区分（见 `sort_models`）。
    pub release_date: Option<String>,
    pub deprecated: bool,
    /// `None` 表示**没有代价数据**，与 `Some(false)`（确认收费）必须可分辨 ——
    /// 界面只在 `Some(true)` 时说 Free，缺数据不编（§7.2）。
    pub free: Option<bool>,
    /// 不可用原因。有值时禁用并显示原因，不从列表里悄悄拿掉（§7.2）。
    pub unavailable: Option<String>,
}

/// 本地偏好。收藏与最近是 Arc Deck 自己的状态，不写回 DSH（§7.2）。
///
/// 强度表的键是 `"provider/model"`。产品名是 **effort**（命名纪律见 §7.4），
/// 上游给这块起的名字只在 `docs/PRODUCT_PLAN.md` 里出现，不落到代码里。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelPrefs {
    pub recent: Vec<ModelRef>,
    pub favorite: Vec<ModelRef>,
    pub effort: BTreeMap<String, String>,
}

impl ModelPrefs {
    /// 放到最前、去重、最多 `RECENT_LIMIT` 条。
    pub fn touch_recent(&mut self, model: &ModelRef) {
        self.recent.retain(|item| item != model);
        self.recent.insert(0, model.clone());
        self.recent.truncate(RECENT_LIMIT);
    }

    /// 有则删，无则插到最前。
    pub fn toggle_favorite(&mut self, model: &ModelRef) {
        match self.favorite.iter().position(|item| item == model) {
            Some(index) => {
                self.favorite.remove(index);
            }
            None => self.favorite.insert(0, model.clone()),
        }
    }

    pub fn effort(&self, model: &ModelRef) -> Option<&str> {
        self.effort.get(&model.route()).map(String::as_str)
    }

    /// `None` 表示跟随模型默认强度，实现方式是删掉这一条键 ——
    /// 不写上游那个 `"default"` 哨兵值（那是它自己 UI 的约定，Arc Deck 的界面还没定）。
    pub fn set_effort(&mut self, model: &ModelRef, effort: Option<&str>) {
        match effort {
            Some(value) => {
                self.effort.insert(model.route(), value.to_string());
            }
            None => {
                self.effort.remove(&model.route());
            }
        }
    }

    /// 读不到、不是 UTF-8、JSON 坏了、字段类型不对 —— 一律回默认值。
    ///
    /// 偏好文件坏掉只该让用户丢掉「最近用过哪些模型」，不该让 TUI 起不来，
    /// 所以这里刻意没有可传播的错误：`load` 的返回类型里就没有失败这一档。
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// 先写临时文件再 rename。
    ///
    /// 理由：直接 `write` 时进程死在写一半，留下的是被截断的 JSON ——
    /// 而 `load` 会把坏 JSON 读成默认值，于是「收藏全没了」这种故障看起来像正常启动。
    /// rename 在同一个目录里做（跨卷 rename 会失败），换上去的是一份完整的文件。
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let name = path
            .file_name()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "偏好文件路径缺少文件名"))?;
        let temp = path.with_file_name(format!("{}.tmp", name.to_string_lossy()));
        let text = serde_json::to_string_pretty(self).map_err(io::Error::other)?;
        std::fs::write(&temp, text)?;
        std::fs::rename(&temp, path)
    }
}

/// 对话框里的一行，字段与上游 `DialogSelect` 的 option 对齐。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelOption {
    pub model: ModelRef,
    pub title: String,
    pub description: Option<String>,
    /// provider 名。上游这里放的是分节名（"Favorites"/"Recent"），Arc Deck 改成 provider 名，
    /// 因为 §7.2 要的是「provider 名作为副标题」，而分节顺序由本函数的输出顺序表达。
    pub category: Option<String>,
    pub footer: Option<String>,
    /// 不可用的原因；`Some` 表示这一行要禁用。
    pub disabled: Option<String>,
}

/// 模型对话框的候选行，顺序即展示顺序。
///
/// * `only_provider` 限定某个 provider（对话框是从 provider 列表跳进来的那个场景）。
/// * `query` 非空时不分 Favorites/Recent，对全部非废弃模型做模糊匹配。
pub fn options(
    providers: &[Provider],
    prefs: &ModelPrefs,
    query: &str,
    only_provider: Option<&str>,
) -> Vec<ModelOption> {
    // 上游先 trim 再判空（`const needle = query().trim()`）：纯空白的搜索框就是空搜索框，
    // 不该被当成一次「搜不到任何东西」的查询。
    let needle = query.trim();
    // 分节只在「没有搜索、也没有限定 provider」时出现（上游 `showExtra`）。
    let sections = needle.is_empty() && only_provider.is_none();

    let groups = provider_groups(providers, prefs, only_provider, sections);
    if !needle.is_empty() {
        return by_score(groups, needle);
    }
    if !sections {
        return groups;
    }

    let mut options = Vec::new();
    for item in &prefs.favorite {
        // 目录里已经没有的引用静默跳过：对话框不是报错的地方（上游同样 `return []`）。
        if let Some((provider, model)) = lookup(providers, item) {
            options.push(row(provider, model, None));
        }
    }
    for item in &prefs.recent {
        // Recent 里已经在 Favorites 出现的不重复列（§7.2）。
        if contains(&prefs.favorite, &item.provider_id, &item.model_id) {
            continue;
        }
        if let Some((provider, model)) = lookup(providers, item) {
            options.push(row(provider, model, None));
        }
    }
    options.extend(groups);
    options
}

/// 按 provider 分组（provider 之间按 name 排序），组内按 `sort_models` 排。
///
/// `sections` 为真时，已经出现在 Favorites/Recent 里的模型不再进分组；
/// 搜索时不排除 —— 搜索要能搜到收藏过的模型（上游同样用 `showSections` 门控这条过滤）。
fn provider_groups(
    providers: &[Provider],
    prefs: &ModelPrefs,
    only_provider: Option<&str>,
    sections: bool,
) -> Vec<ModelOption> {
    let mut ordered: Vec<&Provider> = providers
        .iter()
        .filter(|provider| only_provider.is_none_or(|id| id == provider.id))
        .collect();
    ordered.sort_by(|a, b| a.name.cmp(&b.name));

    let mut options = Vec::new();
    for provider in ordered {
        let mut models: Vec<&Model> = provider
            .models
            .iter()
            // 废弃模型不进 provider 分组。这条刻意不作用于 Favorites/Recent：那是用户
            // 自己写下的引用，收藏的模型后来废弃时留在收藏里至少看得见、能取消收藏，
            // 悄悄消失只会留下一个看不见的死引用。
            .filter(|model| !model.deprecated)
            .filter(|model| {
                !sections
                    || (!contains(&prefs.favorite, &provider.id, &model.id)
                        && !contains(&prefs.recent, &provider.id, &model.id))
            })
            .collect();
        sort_models(&mut models, only_provider.is_some());
        options.extend(models.into_iter().map(|model| {
            let description =
                contains(&prefs.favorite, &provider.id, &model.id).then_some("(Favorite)");
            row(provider, model, description)
        }));
    }
    options
}

/// 组内排序：免费项在前，然后 `release_date` 降序（缺日期排最后），然后 title。
///
/// 限定了 provider 时不看 free —— 上游的 `newestFirst` 分支就是如此，用户已经选定了
/// provider，此时代价不再参与排序。缺日期的排在最后，是因为 `Option` 的序里 `None` 最小，
/// 降序自然把它推到最后。
fn sort_models(models: &mut [&Model], only_provider: bool) {
    models.sort_by(|a, b| {
        if !only_provider {
            let rank = |model: &Model| u8::from(model.free != Some(true));
            if rank(a) != rank(b) {
                return rank(a).cmp(&rank(b));
            }
        }
        b.release_date
            .as_deref()
            .cmp(&a.release_date.as_deref())
            .then_with(|| title(a).cmp(title(b)))
    });
}

/// 模糊匹配：不分节，对分组里的每一行按 title 与 category 取较高分，剔除不匹配的，
/// 按分数降序（排序是稳定的，同分保持 `options` 进来的顺序）。
///
/// 上游这一步之后又调了一次 `sortModelOptions`，结果把 fuzzysort 的分数整个盖掉；
/// 任务书要求的是按分数排，这里不再重排。
fn by_score(options: Vec<ModelOption>, needle: &str) -> Vec<ModelOption> {
    let mut scored: Vec<(i32, ModelOption)> = options
        .into_iter()
        .filter_map(|option| {
            let score = fuzzy_score(needle, &option.title)
                .max(option.category.as_deref().and_then(|c| fuzzy_score(needle, c)));
            score.map(|score| (score, option))
        })
        .collect();
    // `sort_by_key` 稳定，同分保持原顺序。
    scored.sort_by_key(|entry| Reverse(entry.0));
    scored.into_iter().map(|(_, option)| option).collect()
}

/// 忽略大小写的子序列匹配，命中得越整齐分越高；不匹配返回 `None`。
///
/// 自己写是因为构建是 `--offline`，不能为此拉一个模糊匹配 crate（任务书约束）。
/// 逐字符贪心取第一个可能的命中位置，**不回溯**：匹配与否同回溯无关，分数只需要够排序用，
/// 换来的是这条规则能一眼读懂。
fn fuzzy_score(query: &str, haystack: &str) -> Option<i32> {
    const MATCH: i32 = 10;
    const CONSECUTIVE: i32 = 15;
    const WORD_START: i32 = 8;

    let needle: Vec<char> = query.to_lowercase().chars().collect();
    let hay: Vec<char> = haystack.to_lowercase().chars().collect();
    if needle.is_empty() || needle.len() > hay.len() {
        return None;
    }
    let mut score = 0;
    let mut cursor = 0;
    let mut previous: Option<usize> = None;
    for wanted in needle {
        let index = cursor + hay[cursor..].iter().position(|c| *c == wanted)?;
        score += MATCH;
        if let Some(last) = previous
            && last + 1 == index
        {
            score += CONSECUTIVE;
        }
        if index == 0 || is_word_separator(hay[index - 1]) {
            score += WORD_START;
        }
        previous = Some(index);
        cursor = index + 1;
    }
    Some(score)
}

/// 词首分隔符。取的就是模型名与 provider 名里真会出现的那些：
/// `provider/model`、`deepseek-v4.1-flash`、`gpt 4`。
fn is_word_separator(c: char) -> bool {
    matches!(c, '-' | '/' | '.' | ' ')
}

/// 列表里的展示名：`model.name`，缺省回退到 model_id（§7.2）。
fn title(model: &Model) -> &str {
    model.name.as_deref().unwrap_or(&model.id)
}

fn row(provider: &Provider, model: &Model, description: Option<&str>) -> ModelOption {
    ModelOption {
        // 用目录里的 id，不用偏好里那份引用 —— 目录是权威，偏好只是一条记忆。
        model: ModelRef {
            provider_id: provider.id.clone(),
            model_id: model.id.clone(),
        },
        title: title(model).to_string(),
        description: description.map(str::to_string),
        category: Some(provider.name.clone()),
        footer: if model.free == Some(true) {
            Some("Free".to_string())
        } else {
            // `free: None` 是「没有代价数据」，不是免费；这里不编数据（§7.2）。
            None
        },
        disabled: model.unavailable.clone(),
    }
}

fn contains(list: &[ModelRef], provider_id: &str, model_id: &str) -> bool {
    list.iter()
        .any(|item| item.provider_id == provider_id && item.model_id == model_id)
}

fn lookup<'a>(providers: &'a [Provider], reference: &ModelRef) -> Option<(&'a Provider, &'a Model)> {
    let provider = providers
        .iter()
        .find(|provider| provider.id == reference.provider_id)?;
    let model = provider
        .models
        .iter()
        .find(|model| model.id == reference.model_id)?;
    Some((provider, model))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn reference(value: &str) -> ModelRef {
        parse_route(value).expect("夹具里的路由必须合法")
    }

    fn model(id: &str) -> Model {
        Model {
            id: id.to_string(),
            ..Model::default()
        }
    }

    fn provider(id: &str, name: &str, models: Vec<Model>) -> Provider {
        Provider {
            id: id.to_string(),
            name: name.to_string(),
            models,
        }
    }

    fn routes(options: &[ModelOption]) -> Vec<String> {
        options.iter().map(|o| o.model.route()).collect()
    }

    fn find<'a>(options: &'a [ModelOption], route: &str) -> &'a ModelOption {
        options
            .iter()
            .find(|option| option.model.route() == route)
            .expect("候选里应该有这一行")
    }

    /// Alpha 组同时覆盖：免费优先、日期降序、缺日期排最后、按 title 收尾、
    /// 废弃项、不可用项、`name` 缺省回退到 id。
    fn catalog() -> Vec<Provider> {
        let mut free_old = model("alpha-flash");
        free_old.free = Some(true);
        free_old.release_date = Some("2025-01-01".to_string());

        let mut paid_new = model("alpha-pro");
        paid_new.free = Some(false);
        paid_new.release_date = Some("2026-01-01".to_string());

        let mut unknown_cost = model("alpha-lite");
        unknown_cost.release_date = Some("2026-02-01".to_string());

        let mut deprecated = model("alpha-legacy");
        deprecated.deprecated = true;
        deprecated.release_date = Some("2026-03-01".to_string());

        let mut offline = model("alpha-offline");
        offline.unavailable = Some("no credentials".to_string());

        let mut named = model("alpha-named");
        named.name = Some("Alpha Named".to_string());

        vec![
            provider(
                "zeta",
                "Zeta",
                vec![{
                    let mut zed = model("zeta-one");
                    zed.release_date = Some("2026-05-01".to_string());
                    zed
                }],
            ),
            provider("deepseek", "DeepSeek", vec![model("deepseek-v4.1-flash")]),
            provider(
                "alpha",
                "Alpha",
                vec![named, unknown_cost, paid_new, free_old, deprecated, offline],
            ),
        ]
    }

    #[test]
    fn route_splits_on_the_first_slash_only() {
        let value = parse_route("a/b/c").expect("a/b/c 合法");
        assert_eq!(value.provider_id, "a");
        assert_eq!(value.model_id, "b/c");
        // 拼回去要还原成同一条路由（effort 的键就靠这条不变式）。
        assert_eq!(value.route(), "a/b/c");
        assert!(parse_route("a").is_none());
        assert!(parse_route("/b").is_none());
        assert!(parse_route("a/").is_none());
        assert!(parse_route("").is_none());
    }

    #[test]
    fn recent_keeps_ten_and_drops_duplicates() {
        let mut prefs = ModelPrefs::default();
        for index in 0..12 {
            prefs.touch_recent(&reference(&format!("p/m{index}")));
        }
        assert_eq!(prefs.recent.len(), RECENT_LIMIT);
        assert_eq!(prefs.recent[0], reference("p/m11"));
        assert_eq!(prefs.recent[RECENT_LIMIT - 1], reference("p/m2"));

        prefs.touch_recent(&reference("p/m5"));
        assert_eq!(prefs.recent[0], reference("p/m5"));
        assert_eq!(prefs.recent.len(), RECENT_LIMIT);
        assert_eq!(
            prefs
                .recent
                .iter()
                .filter(|item| **item == reference("p/m5"))
                .count(),
            1,
            "重复的模型只能出现一次"
        );
    }

    #[test]
    fn toggle_favorite_inserts_at_the_front_and_removes_on_the_second_call() {
        let mut prefs = ModelPrefs::default();
        prefs.toggle_favorite(&reference("p/a"));
        prefs.toggle_favorite(&reference("p/b"));
        assert_eq!(prefs.favorite, vec![reference("p/b"), reference("p/a")]);
        prefs.toggle_favorite(&reference("p/a"));
        assert_eq!(prefs.favorite, vec![reference("p/b")]);
    }

    #[test]
    fn effort_is_keyed_by_provider_and_model() {
        let mut prefs = ModelPrefs::default();
        prefs.set_effort(&reference("a/b/c"), Some("high"));
        assert_eq!(prefs.effort.get("a/b/c").map(String::as_str), Some("high"));
        assert_eq!(prefs.effort(&reference("a/b/c")), Some("high"));
        assert_eq!(prefs.effort(&reference("a/b")), None);
        prefs.set_effort(&reference("a/b/c"), None);
        assert!(prefs.effort.is_empty());
    }

    #[test]
    fn groups_are_sorted_by_provider_then_free_then_release_date_then_title() {
        let options = options(&catalog(), &ModelPrefs::default(), "", None);
        assert_eq!(
            routes(&options),
            vec![
                "alpha/alpha-flash",   // free == Some(true)，代价优先于日期
                "alpha/alpha-lite",    // 2026-02-01
                "alpha/alpha-pro",     // 2026-01-01
                "alpha/alpha-named",   // 无日期，title "Alpha Named"
                "alpha/alpha-offline", // 无日期，title 回退到 id
                "deepseek/deepseek-v4.1-flash",
                "zeta/zeta-one",
            ]
        );
        assert_eq!(find(&options, "alpha/alpha-named").title, "Alpha Named");
        assert_eq!(find(&options, "alpha/alpha-offline").title, "alpha-offline");
        // category 是 provider 名（不是上游的分节名）。
        assert_eq!(find(&options, "zeta/zeta-one").category.as_deref(), Some("Zeta"));
    }

    #[test]
    fn deprecated_models_never_appear() {
        let prefs = ModelPrefs::default();
        for (query, only) in [("", None), ("legacy", None), ("", Some("alpha"))] {
            let options = options(&catalog(), &prefs, query, only);
            assert!(
                !routes(&options).contains(&"alpha/alpha-legacy".to_string()),
                "废弃模型出现在 query={query:?} only={only:?} 的列表里"
            );
        }
    }

    #[test]
    fn free_none_has_no_footer_and_free_true_says_free() {
        let options = options(&catalog(), &ModelPrefs::default(), "", None);
        assert_eq!(
            find(&options, "alpha/alpha-flash").footer.as_deref(),
            Some("Free")
        );
        assert_eq!(find(&options, "alpha/alpha-lite").footer, None);
        assert_eq!(find(&options, "alpha/alpha-pro").footer, None);
    }

    #[test]
    fn unavailable_models_stay_in_the_list_with_their_reason() {
        let listed = options(&catalog(), &ModelPrefs::default(), "", None);
        assert_eq!(
            find(&listed, "alpha/alpha-offline").disabled.as_deref(),
            Some("no credentials")
        );
        assert_eq!(find(&listed, "alpha/alpha-lite").disabled, None);
        // 搜索也要能找到它 —— 不可用不等于不存在。
        let searched = options(&catalog(), &ModelPrefs::default(), "offline", None);
        assert_eq!(
            find(&searched, "alpha/alpha-offline").disabled.as_deref(),
            Some("no credentials")
        );
    }

    #[test]
    fn favorites_come_first_then_recent_without_repeats() {
        let mut prefs = ModelPrefs::default();
        prefs.toggle_favorite(&reference("alpha/alpha-pro"));
        // 目录里已经不存在的引用：静默跳过，不报错也不占位。
        prefs.toggle_favorite(&reference("ghost/nope"));
        prefs.touch_recent(&reference("alpha/alpha-flash"));
        prefs.touch_recent(&reference("alpha/alpha-pro"));
        prefs.touch_recent(&reference("ghost/gone"));

        let options = options(&catalog(), &prefs, "", None);
        let listed = routes(&options);
        assert_eq!(listed[0], "alpha/alpha-pro", "收藏在最前");
        assert_eq!(listed[1], "alpha/alpha-flash", "最近用过排第二");
        assert!(
            !listed.iter().any(|route| route.starts_with("ghost/")),
            "目录里没有的引用不该出现在候选里"
        );
        // Favorites 与 Recent 不重复，分组里也不再出现已列过的两个模型。
        let mut seen = std::collections::BTreeSet::new();
        for route in &listed {
            assert!(seen.insert(route.clone()), "{route} 被列了两次");
        }
    }

    #[test]
    fn favorites_are_marked_inside_provider_lists() {
        let mut prefs = ModelPrefs::default();
        prefs.toggle_favorite(&reference("alpha/alpha-flash"));
        // 限定 provider 时收藏项仍在分组里，此时用 "(Favorite)" 标出来。
        let limited = options(&catalog(), &prefs, "", Some("alpha"));
        assert_eq!(
            find(&limited, "alpha/alpha-flash").description.as_deref(),
            Some("(Favorite)")
        );
        assert_eq!(find(&limited, "alpha/alpha-lite").description, None);

        // 搜索同样标记；Favorites 不再单独列出，所以这个模型在结果里只出现一次
        // （"flash" 同时命中 deepseek-v4.1-flash，所以只数这一条）。
        let searched = options(&catalog(), &prefs, "flash", None);
        assert_eq!(
            routes(&searched)
                .iter()
                .filter(|route| *route == "alpha/alpha-flash")
                .count(),
            1
        );
        assert_eq!(
            find(&searched, "alpha/alpha-flash").description.as_deref(),
            Some("(Favorite)")
        );
    }

    #[test]
    fn limiting_a_provider_ignores_free_and_drops_the_sections() {
        let mut prefs = ModelPrefs::default();
        prefs.toggle_favorite(&reference("alpha/alpha-pro"));
        let limited = options(&catalog(), &prefs, "", Some("alpha"));
        assert_eq!(
            routes(&limited),
            vec![
                "alpha/alpha-lite",    // 2026-02-01
                "alpha/alpha-pro",     // 2026-01-01（被收藏，但这里只按日期排）
                "alpha/alpha-flash",   // 2025-01-01：free 在这里不参与排序
                "alpha/alpha-named",
                "alpha/alpha-offline",
            ]
        );
        // 限定 provider 时不列 Favorites/Recent 分节，收藏项靠 "(Favorite)" 认出来。
        assert_eq!(limited[0].model.route(), "alpha/alpha-lite");
    }

    #[test]
    fn query_matches_title_and_category_case_insensitively() {
        let prefs = ModelPrefs::default();
        let short = options(&catalog(), &prefs, "v41f", None);
        assert_eq!(routes(&short), vec!["deepseek/deepseek-v4.1-flash"]);

        let upper = options(&catalog(), &prefs, "V41F", None);
        assert_eq!(routes(&upper), routes(&short), "大小写不该改变结果");

        // category 是 provider 名，所以搜 provider 名能出它名下的模型。
        let by_provider = options(&catalog(), &prefs, "zeta", None);
        assert_eq!(routes(&by_provider), vec!["zeta/zeta-one"]);

        // 空白查询按空查询处理（回到分节分组），而不是「什么都搜不到」。
        let blank = options(&catalog(), &prefs, "   ", None);
        assert!(routes(&blank).contains(&"zeta/zeta-one".to_string()));
    }

    #[test]
    fn query_without_a_match_returns_nothing() {
        let mut prefs = ModelPrefs::default();
        prefs.toggle_favorite(&reference("alpha/alpha-flash"));
        prefs.touch_recent(&reference("alpha/alpha-pro"));
        assert!(options(&catalog(), &prefs, "qqqq", None).is_empty());
        // 收藏与最近也不该绕过匹配规则单独出现。
        assert!(options(&catalog(), &prefs, "ghost", None).is_empty());
    }

    #[test]
    fn search_ranks_consecutive_word_start_matches_first() {
        let providers = vec![provider(
            "p",
            "P",
            vec![
                {
                    let mut spaced = model("f.l.a.s.h");
                    spaced.release_date = Some("2026-01-01".to_string());
                    spaced
                },
                model("flash"),
            ],
        )];
        // 分组顺序是日期降序，"f.l.a.s.h" 在前；分数排序要把它换到后面去。
        let options = options(&providers, &ModelPrefs::default(), "fl", None);
        assert_eq!(routes(&options), vec!["p/flash", "p/f.l.a.s.h"]);
        assert!(fuzzy_score("fl", "flash") > fuzzy_score("fl", "f.l.a.s.h"));
    }

    #[test]
    fn equal_scores_keep_the_group_order() {
        let providers = vec![provider(
            "p",
            "P",
            vec![
                {
                    let mut beta = model("beta");
                    beta.release_date = Some("2025-01-01".to_string());
                    beta
                },
                {
                    let mut delta = model("delta");
                    delta.release_date = Some("2026-01-01".to_string());
                    delta
                },
            ],
        )];
        // 两个 title 里 "e" 的分数相同（都在词中，10 分），排序后必须仍是分组顺序
        // （delta 的日期更新，本来就在前），不能被吹成字母序（beta 在前）。
        assert_eq!(fuzzy_score("e", "beta"), fuzzy_score("e", "delta"));
        let options = options(&providers, &ModelPrefs::default(), "e", None);
        assert_eq!(routes(&options), vec!["p/delta", "p/beta"]);
    }

    #[test]
    fn fuzzy_score_rejects_non_subsequences() {
        assert_eq!(fuzzy_score("zz", "alpha"), None);
        // "alpha" 里的 h 在最后一个 a 之前，所以 hl 没有可用的命中顺序。
        assert_eq!(fuzzy_score("hl", "alpha"), None, "顺序也要对");
        assert!(fuzzy_score("ap", "alpha").is_some());
        assert_eq!(fuzzy_score("", "alpha"), None);
        assert_eq!(fuzzy_score("alphabet", "alpha"), None, "查询比目标还长");
    }

    #[test]
    fn load_falls_back_to_defaults_for_missing_or_broken_files() {
        let dir = tempdir().expect("临时目录");
        let path = dir.path().join("model.json");
        assert_eq!(ModelPrefs::load(&path), ModelPrefs::default());

        std::fs::write(&path, "{ this is not json").expect("写入坏 JSON");
        assert_eq!(ModelPrefs::load(&path), ModelPrefs::default());

        // 顶层类型不对同样回默认值，不 panic。
        std::fs::write(&path, "[]").expect("写入数组");
        assert_eq!(ModelPrefs::load(&path), ModelPrefs::default());

        // 字段缺失按空处理，已有的字段要留下。
        std::fs::write(&path, r#"{"effort":{"a/b":"high"}}"#).expect("写入部分字段");
        let partial = ModelPrefs::load(&path);
        assert!(partial.recent.is_empty());
        assert!(partial.favorite.is_empty());
        assert_eq!(partial.effort.get("a/b").map(String::as_str), Some("high"));
    }

    #[test]
    fn save_then_load_round_trips_and_leaves_no_temp_file() {
        let dir = tempdir().expect("临时目录");
        // 父目录不存在也要能存：状态目录第一次运行时的情形。
        let path = dir.path().join("nested").join("model.json");
        let mut prefs = ModelPrefs::default();
        prefs.touch_recent(&reference("a/b/c"));
        prefs.toggle_favorite(&reference("deepseek/deepseek-v4-pro"));
        prefs.set_effort(&reference("a/b/c"), Some("high"));
        prefs.save(&path).expect("保存偏好");
        assert_eq!(ModelPrefs::load(&path), prefs);

        let names: Vec<String> = std::fs::read_dir(path.parent().expect("父目录"))
            .expect("列出目录")
            .map(|entry| entry.expect("目录项").file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["model.json".to_string()], "临时文件必须被 rename 掉");

        // 再存一次（内容变短）：rename 覆盖，不能留下上一次的尾巴。
        prefs.toggle_favorite(&reference("deepseek/deepseek-v4-pro"));
        prefs.set_effort(&reference("a/b/c"), None);
        prefs.save(&path).expect("覆盖保存");
        assert_eq!(ModelPrefs::load(&path), prefs);
    }
}
