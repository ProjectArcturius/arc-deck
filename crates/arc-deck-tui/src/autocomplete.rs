//! A generic completion popup over the composer, plus the `/` command provider.
//!
//! The popup is deliberately not "the slash command list": it owns a trigger character, a
//! filtered candidate list and a cursor. A later task feeds `@` file candidates through the same
//! `Item` shape and the same keys (↑↓ / Tab / Enter / Esc) without touching this state machine.
//!
//! Behaviour follows OpenCode v1.19.0 `component/prompt/autocomplete.tsx`: the popup is visible
//! only while the text after the trigger has no whitespace, typing filters, ↑↓ move, Tab/Enter
//! accept, Esc closes the popup without touching the draft.

use crate::commands::{self, Action};
use crate::keymap::Keymap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Item {
    /// What accepting puts into the editor. Commands whose argument is **required** keep a
    /// trailing space so the next keystroke is the argument; the rest run on Enter.
    pub insert: String,
    pub title: String,
    /// Right-aligned detail: the slash name plus its argument hint.
    pub detail: String,
    pub available: bool,
    pub reason: Option<String>,
    /// Slash commands carry their action; file references do not (nothing to run).
    pub action: Option<Action>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Autocomplete {
    pub trigger: char,
    /// Byte index in the draft where the trigger char starts. Accepting replaces
    /// `draft[start..cursor]`; for `/` that is 0, for `@` it is the `@` itself.
    pub start: usize,
    pub query: String,
    pub items: Vec<Item>,
    pub selected: usize,
}

impl Autocomplete {
    pub(crate) fn new(trigger: char, start: usize, query: String, items: Vec<Item>) -> Option<Self> {
        if items.is_empty() {
            return None;
        }
        Some(Self {
            trigger,
            start,
            query,
            items,
            selected: 0,
        })
    }

    pub(crate) fn move_by(&mut self, delta: isize) {
        if self.items.is_empty() {
            return;
        }
        let len = self.items.len() as isize;
        self.selected = (self.selected as isize + delta).clamp(0, len - 1) as usize;
    }

    pub(crate) fn current(&self) -> Option<&Item> {
        self.items.get(self.selected)
    }
}

/// The text after the trigger, when the trigger starts the draft and no argument has begun.
/// `None` hides the popup: once the user types a space they are writing an argument, not a name.
pub(crate) fn trigger_query(text: &str, trigger: char) -> Option<&str> {
    let rest = text.strip_prefix(trigger)?;
    if rest.chars().any(char::is_whitespace) {
        return None;
    }
    Some(rest)
}

/// Candidates for `/` completion. Unavailable commands stay listed with their reason — hiding
/// them would make the reason undiscoverable, which is the opposite of §7.4.
pub(crate) fn slash_items(
    query: &str,
    keymap: &Keymap,
    availability: &dyn Fn(Action) -> Option<String>,
) -> Vec<Item> {
    let mut scored: Vec<(i32, usize, Item)> = Vec::new();
    for (index, entry) in commands::ENTRIES.iter().enumerate() {
        let Some(slash) = entry.slash else {
            continue;
        };
        let name = slash.trim_start_matches('/');
        let Some(score) = fuzzy(query, name).or_else(|| fuzzy(query, entry.title)) else {
            continue;
        };
        // A command bound to a key still shows the key: the popup is where users learn it.
        let shortcut = keymap
            .display(entry.action)
            .map(|keys| format!(" · {keys}"))
            .unwrap_or_default();
        let reason = entry
            .unavailable
            .map(str::to_string)
            .or_else(|| availability(entry.action));
        let available = reason.is_none();
        let detail = match (entry.arg, available) {
            (Some(arg), _) => format!("{slash} {arg}{shortcut}"),
            (None, _) => format!("{slash}{shortcut}"),
        };
        scored.push((
            score,
            index,
            Item {
                // 末尾空格 = 「等参数，别执行」：只给必需参数的命令（见 `Entry::arg_required`）。
                insert: if entry.arg_required() { format!("{slash} ") } else { slash.to_string() },
                title: entry.title.to_string(),
                detail,
                available,
                reason,
                action: Some(entry.action),
            },
        ));
    }
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, _, item)| item).collect()
}

/// Subsequence match with a small quality score: consecutive hits and word boundaries rank
/// higher, so `th` prefers `theme` over `tools-help`-style scattered matches.
pub(crate) fn fuzzy(query: &str, target: &str) -> Option<i32> {
    let query = query.to_lowercase();
    if query.is_empty() {
        return Some(0);
    }
    let target = target.to_lowercase();
    let mut score = 0;
    let mut previous: Option<usize> = None;
    let mut chars = target.char_indices();
    for needle in query.chars() {
        let (index, _) = chars.find(|(_, hay)| *hay == needle)?;
        score += 1;
        if previous.is_some_and(|previous| previous + 1 == index) {
            score += 2;
        }
        if index == 0
            || target[..index]
                .chars()
                .next_back()
                .is_some_and(|c| matches!(c, '.' | '-' | '_' | '/' | ' '))
        {
            score += 2;
        }
        previous = Some(index);
    }
    Some(score)
}

/// `@` reference query: the trigger may sit anywhere in the draft, but only in the word being
/// typed (the text after the last whitespace before the cursor). Returns the byte index of the
/// trigger and the query after it.
///
/// Unlike `/`, an `@` inside an email address must not open the popup, so the trigger has to
/// start the word.
pub(crate) fn mention_query(text: &str, cursor: usize, trigger: char) -> Option<(usize, &str)> {
    let cursor = cursor.min(text.len());
    if !text.is_char_boundary(cursor) {
        return None;
    }
    let upto = &text[..cursor];
    let start = upto
        .rfind(char::is_whitespace)
        .map(|index| index + 1)
        .unwrap_or(0);
    let word = &upto[start..];
    let rest = word.strip_prefix(trigger)?;
    // A second trigger in the word means the user typed it as text (`@a@b`), not a reference.
    if rest.contains(trigger) || rest.chars().any(char::is_whitespace) {
        return None;
    }
    Some((start, rest))
}

/// File candidates for the word after `@`. `insert` keeps a trailing space so the next
/// keystroke starts a new word instead of extending the path.
pub(crate) fn file_items(
    index: &crate::files::FileIndex,
    query: &str,
    recent: &[String],
    limit: usize,
) -> Vec<Item> {
    index
        .matches(query, recent, limit)
        .into_iter()
        .map(|path| Item {
            insert: format!("@{path} "),
            // The path is the whole row; there is no second label to show.
            title: String::new(),
            detail: format!("@{path}"),
            available: true,
            reason: None,
            action: None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keymap() -> Keymap {
        Keymap::defaults()
    }

    fn available(_: Action) -> Option<String> {
        None
    }

    #[test]
    fn the_trigger_only_counts_before_the_first_space() {
        assert_eq!(trigger_query("/", '/'), Some(""));
        assert_eq!(trigger_query("/th", '/'), Some("th"));
        assert_eq!(trigger_query("/effort high", '/'), None, "arguments are not a query");
        assert_eq!(trigger_query(" /th", '/'), None, "the trigger must start the draft");
        assert_eq!(trigger_query("hello", '/'), None);
        assert_eq!(trigger_query("/a\nb", '/'), None);
        assert_eq!(trigger_query("@src", '@'), Some("src"));
    }

    /// `@` works mid-draft, but only at the start of the word under the cursor.
    #[test]
    fn a_mention_trigger_is_scoped_to_the_current_word() {
        assert_eq!(mention_query("看看 @src/ma", 14, '@'), Some((7, "src/ma")));
        assert_eq!(mention_query("看看 @src/ma 后续", 14, '@'), Some((7, "src/ma")));
        assert_eq!(mention_query("user@example.com", 16, '@'), None, "emails are not mentions");
        assert_eq!(mention_query("@a@b", 4, '@'), None, "a second trigger is just text");
        assert_eq!(mention_query("没有引用", 5, '@'), None);
        assert_eq!(mention_query("@中", 4, '@'), Some((0, "中")));
    }

    #[test]
    fn empty_query_lists_every_slash_command_in_registry_order() {
        let items = slash_items("", &keymap(), &available);
        let expected = commands::ENTRIES.iter().filter(|e| e.slash.is_some()).count();
        assert_eq!(items.len(), expected);
        assert_eq!(items[0].insert, "/new");
        // The palette has no slash, so it is not a completion candidate.
        assert!(items.iter().all(|item| item.action != Some(Action::Palette)));
    }

    #[test]
    fn filtering_prefers_prefix_and_boundary_matches() {
        let items = slash_items("th", &keymap(), &available);
        assert_eq!(items[0].insert, "/theme", "`th` should find /theme first: {:?}", items);
        assert!(
            !items.iter().any(|item| item.insert == "/tools"),
            "`th` is not a subsequence of `tools`"
        );
        let items = slash_items("to", &keymap(), &available);
        assert_eq!(items[0].insert, "/tools", "a prefix match ranks first: {:?}", items);
    }

    /// 不可用行仍要**列出来**并带原因 —— 但原因来自调用方的 `availability`，
    /// 而不是注册表里写死的一条（Plan 接通之后注册表里已经没有恒不可用的行了）。
    #[test]
    fn unavailable_commands_stay_listed_with_their_reason() {
        let items = slash_items("plan", &keymap(), &|action| {
            (action == Action::Plan).then(|| "现在不能切：这一轮还在进行".to_string())
        });
        let plan = items.iter().find(|item| item.action == Some(Action::Plan)).unwrap();
        assert!(!plan.available);
        assert!(plan.reason.as_deref().unwrap().contains("这一轮还在进行"));
    }
    #[test]
    fn state_dependent_availability_comes_from_the_caller() {
        let items = slash_items("cancel", &keymap(), &|action| {
            (action == Action::Cancel).then(|| "当前没有活动运行".to_string())
        });
        let cancel = items.iter().find(|item| item.action == Some(Action::Cancel)).unwrap();
        assert!(!cancel.available);
        assert_eq!(cancel.reason.as_deref(), Some("当前没有活动运行"));
    }

    #[test]
    fn only_required_arguments_keep_a_trailing_space() {
        // 可选参数（`[…]`）：不带空格，Enter 直接执行（没参数时打开选择器）。
        let items = slash_items("effort", &keymap(), &available);
        let effort = items.iter().find(|item| item.action == Some(Action::Effort)).unwrap();
        assert_eq!(effort.insert, "/effort");
        assert!(effort.detail.contains("[级别|default]"));
        // 必需参数（`<…>`）：带空格，等参数。
        let items = slash_items("search", &keymap(), &available);
        let search = items.iter().find(|item| item.action == Some(Action::Search)).unwrap();
        assert_eq!(search.insert, "/search ");
    }

    #[test]
    fn selection_is_clamped_and_never_empty_when_open() {
        let mut popup = Autocomplete::new('/', 0, String::new(), slash_items("", &keymap(), &available))
            .unwrap();
        popup.move_by(-1);
        assert_eq!(popup.selected, 0);
        popup.move_by(10_000);
        assert_eq!(popup.selected, popup.items.len() - 1);
        assert!(popup.current().is_some());
        assert!(Autocomplete::new('/', 0, String::new(), Vec::new()).is_none());
    }

    #[test]
    fn fuzzy_ranking_is_stable() {
        assert_eq!(fuzzy("", "anything"), Some(0));
        assert!(fuzzy("th", "theme").unwrap() > fuzzy("th", "together-here").unwrap());
        assert_eq!(fuzzy("zz", "theme"), None);
        assert!(fuzzy("nw", "new").is_some(), "subsequences are enough");
    }
}
