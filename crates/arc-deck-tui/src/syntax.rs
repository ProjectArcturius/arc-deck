//! 代码块的着色。**照抄 OpenCode 的做法，不自己发明配色**：颜色全部来自主题的
//! `syntax` token 组（`crates/arc-deck-tui/themes/*.json` 的 `syntax`），
//! 也就是 OpenCode 自己那套（keyword / string / comment / number / type / function /
//! variable / operator / punctuation）。
//!
//! 为什么自己写一个行级扫描器而不是引 Shiki：Shiki 是 TS 生态、带整套语法定义与
//! WASM/正则引擎，为 TUI 里几十行代码引它是把「渲染」变成「第二个运行时」。
//! 这里只做 OpenCode TUI 观感所需的那一层：关键字、字符串、注释、数字、类型、
//! 函数名、运算符。**认不出的语言一律不着色** —— 猜错比不猜更难看。
//!
//! 状态跨行只有两种：`/* … */` 块注释与三引号字符串（Python）。没有它们，多行注释
//! 的每一行都会被重新当成代码，颜色在第一行之后就散架。

use ratatui::style::Color;

/// 一行的着色结果：按顺序拼回去就是原文（一个字都不改）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Token {
    Plain,
    Comment,
    Keyword,
    Function,
    Type,
    String,
    Number,
    Variable,
    Operator,
    Punctuation,
}

/// 主题 `syntax.*` 的取色表。缺的 token 落到正文色（`plain`），不编颜色。
#[derive(Debug, Clone, Copy)]
pub(crate) struct SyntaxPalette {
    pub plain: Color,
    pub comment: Color,
    pub keyword: Color,
    pub function: Color,
    pub r#type: Color,
    pub string: Color,
    pub number: Color,
    pub variable: Color,
    pub operator: Color,
    pub punctuation: Color,
}

impl SyntaxPalette {
    pub(crate) fn color(&self, token: Token) -> Color {
        match token {
            Token::Plain => self.plain,
            Token::Comment => self.comment,
            Token::Keyword => self.keyword,
            Token::Function => self.function,
            Token::Type => self.r#type,
            Token::String => self.string,
            Token::Number => self.number,
            Token::Variable => self.variable,
            Token::Operator => self.operator,
            Token::Punctuation => self.punctuation,
        }
    }
}

/// 跨行状态。
#[derive(Debug, Clone, Default)]
pub(crate) struct Highlighter {
    /// 在 `/* … */` 里。
    block_comment: bool,
    /// 在 `"""` / `'''` 里（Python 的多行字符串）。
    triple: Option<&'static str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    Rust,
    C,
    Shell,
    Python,
    Json,
    Toml,
    Sql,
    Markup,
    Unknown,
}

fn family(language: &str) -> Family {
    match language.to_ascii_lowercase().as_str() {
        "rs" | "rust" => Family::Rust,
        "c" | "h" | "cpp" | "c++" | "cc" | "hpp" | "java" | "kt" | "kotlin" | "cs" | "csharp"
        | "go" | "ts" | "tsx" | "typescript" | "js" | "jsx" | "javascript" | "mjs" | "cjs" | "swift"
        | "scala" | "dart" => Family::C,
        "sh" | "bash" | "zsh" | "shell" | "console" | "powershell" | "ps1" | "ps" => Family::Shell,
        "py" | "python" | "pyi" => Family::Python,
        "json" | "jsonc" => Family::Json,
        "toml" | "ini" | "yaml" | "yml" => Family::Toml,
        "sql" => Family::Sql,
        "html" | "xml" | "svg" | "css" | "scss" | "md" | "markdown" => Family::Markup,
        _ => Family::Unknown,
    }
}

impl Family {
    /// 行注释的起始串。`None` 表示这类语言没有行注释（或这里不认）。
    fn line_comment(self) -> &'static [&'static str] {
        match self {
            Family::Rust | Family::C => &["//"],
            Family::Python | Family::Shell | Family::Toml => &["#"],
            Family::Sql => &["--"],
            Family::Json | Family::Markup | Family::Unknown => &[],
        }
    }

    fn has_block_comment(self) -> bool {
        matches!(self, Family::Rust | Family::C | Family::Sql)
    }

    fn keywords(self) -> &'static [&'static str] {
        match self {
            Family::Rust => &[
                "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum",
                "extern", "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod",
                "move", "mut", "pub", "ref", "return", "self", "Self", "static", "struct", "super",
                "trait", "true", "type", "unsafe", "use", "where", "while", "crate", "macro_rules",
            ],
            Family::C => &[
                "abstract", "as", "async", "await", "break", "case", "catch", "class", "const",
                "continue", "default", "delete", "do", "else", "enum", "export", "extends", "false",
                "final", "finally", "for", "from", "function", "func", "go", "if", "implements",
                "import", "in", "instanceof", "interface", "let", "map", "new", "null", "package",
                "private", "protected", "public", "return", "static", "struct", "super", "switch",
                "this", "throw", "throws", "true", "try", "type", "typeof", "undefined", "var",
                "void", "while", "yield", "defer", "fn", "impl", "pub", "use", "match",
            ],
            Family::Shell => &[
                "if", "then", "else", "elif", "fi", "for", "while", "do", "done", "case", "esac",
                "function", "in", "select", "until", "local", "export", "return", "exit", "set",
                "unset", "echo", "cd", "source", "alias",
            ],
            Family::Python => &[
                "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del",
                "elif", "else", "except", "False", "finally", "for", "from", "global", "if", "import",
                "in", "is", "lambda", "None", "nonlocal", "not", "or", "pass", "raise", "return",
                "True", "try", "while", "with", "yield", "self", "match", "case",
            ],
            Family::Json => &["true", "false", "null"],
            Family::Toml => &["true", "false"],
            Family::Sql => &[
                "select", "from", "where", "insert", "into", "values", "update", "set", "delete",
                "create", "table", "drop", "alter", "join", "left", "right", "inner", "outer", "on",
                "group", "by", "order", "having", "limit", "offset", "and", "or", "not", "null",
                "as", "distinct", "case", "when", "then", "else", "end", "primary", "key", "index",
            ],
            Family::Markup | Family::Unknown => &[],
        }
    }
}

/// 切一行。`highlighter` 带着跨行的注释 / 三引号状态，**同一段代码要按顺序复用同一个**。
pub(crate) fn line(highlighter: &mut Highlighter, text: &str, language: &str) -> Vec<(String, Token)> {
    let family = family(language);
    if family == Family::Unknown {
        return vec![(text.to_string(), Token::Plain)];
    }
    let mut out: Vec<(String, Token)> = Vec::new();
    let mut current = String::new();
    let mut current_token = Token::Plain;
    // 把攒着的同色字符一次性推出去：一行代码通常只有几十个 span，不必再细分。
    macro_rules! flush {
        () => {
            if !current.is_empty() {
                out.push((std::mem::take(&mut current), current_token));
            }
        };
    }

    let chars: Vec<char> = text.chars().collect();
    let mut index = 0;
    while index < chars.len() {
        // 三引号字符串的续行：整行都是字符串，直到再次出现同一个三引号。
        if let Some(delimiter) = highlighter.triple {
            let rest: String = chars[index..].iter().collect();
            if let Some(position) = rest.find(delimiter) {
                let end = position + delimiter.len();
                current.push_str(&rest[..end]);
                index += rest[..end].chars().count();
                current_token = Token::String;
                flush!();
                highlighter.triple = None;
                continue;
            }
            current_token = Token::String;
            current.extend(chars[index..].iter());
            index = chars.len();
            flush!();
            continue;
        }
        if highlighter.block_comment {
            let rest: String = chars[index..].iter().collect();
            if let Some(position) = rest.find("*/") {
                let end = position + 2;
                current_token = Token::Comment;
                current.push_str(&rest[..end]);
                index += rest[..end].chars().count();
                flush!();
                highlighter.block_comment = false;
                continue;
            }
            current_token = Token::Comment;
            current.extend(chars[index..].iter());
            index = chars.len();
            flush!();
            continue;
        }

        // 行注释：到行尾。
        let rest: String = chars[index..].iter().collect();
        if let Some(marker) = family.line_comment().iter().find(|marker| rest.starts_with(**marker)) {
            // `#` 在 Rust 里是属性（`#[derive]`），不是注释。
            if *marker == "#" && family == Family::Rust {
                current.push('#');
                index += 1;
                continue;
            }
            current_token = Token::Comment;
            current.push_str(&rest);
            index = chars.len();
            let _ = marker;
            flush!();
            continue;
        }
        // 块注释开始。
        if family.has_block_comment() && rest.starts_with("/*") {
            current_token = Token::Comment;
            current.push_str("/*");
            index += 2;
            highlighter.block_comment = true;
            flush!();
            continue;
        }
        // 三引号字符串（Python）。
        if family == Family::Python && (rest.starts_with("\"\"\"") || rest.starts_with("'''")) {
            let delimiter = if rest.starts_with("\"\"\"") { "\"\"\"" } else { "'''" };
            current_token = Token::String;
            current.push_str(delimiter);
            index += 3;
            let after: String = chars[index..].iter().collect();
            if let Some(position) = after.find(delimiter) {
                current.push_str(&after[..position + 3]);
                index += after[..position + 3].chars().count();
            } else {
                highlighter.triple = Some(delimiter);
                current.extend(chars[index..].iter());
                index = chars.len();
            }
            flush!();
            continue;
        }
        // 普通字符串：单行，支持反斜杠转义。
        let c = chars[index];
        if c == '"' || c == '\'' || (c == '`' && family == Family::C) {
            current_token = Token::String;
            current.push(c);
            index += 1;
            while index < chars.len() {
                let ch = chars[index];
                current.push(ch);
                index += 1;
                if ch == '\\' && index < chars.len() {
                    current.push(chars[index]);
                    index += 1;
                    continue;
                }
                if ch == c {
                    break;
                }
            }
            flush!();
            continue;
        }
        // 数字。
        if c.is_ascii_digit() {
            current_token = Token::Number;
            while index < chars.len() {
                let ch = chars[index];
                if ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' {
                    current.push(ch);
                    index += 1;
                } else {
                    break;
                }
            }
            flush!();
            continue;
        }
        // 标识符 / 关键字 / 类型 / 函数。
        if c.is_alphabetic() || c == '_' {
            let start = index;
            while index < chars.len() && (chars[index].is_alphanumeric() || chars[index] == '_') {
                index += 1;
            }
            let word: String = chars[start..index].iter().collect();
            let token = if family.keywords().iter().any(|keyword| *keyword == word) {
                Token::Keyword
            } else if word.chars().next().is_some_and(char::is_uppercase) {
                Token::Type
            } else {
                let mut look = index;
                while look < chars.len() && chars[look] == ' ' {
                    look += 1;
                }
                if look < chars.len() && chars[look] == '(' {
                    Token::Function
                } else if family == Family::Shell && word.chars().all(|ch| ch.is_ascii_uppercase() || ch == '_') {
                    Token::Variable
                } else {
                    Token::Plain
                }
            };
            current.push_str(&word);
            current_token = token;
            flush!();
            continue;
        }
        // 运算符与标点：分开着色，括号不该跟 `+` 同色。
        let token = if "+-*/%=<>!&|^~?".contains(c) {
            Token::Operator
        } else if "(){}[];,.:@#".contains(c) {
            Token::Punctuation
        } else {
            Token::Plain
        };
        current.push(c);
        current_token = token;
        index += 1;
        flush!();
    }
    flush!();
    // 空白不该单独成 span —— 拼回去仍然是原文，但空段会让渲染多做一次分配。
    out.retain(|(text, _)| !text.is_empty());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(text: &str, language: &str) -> Vec<(String, Token)> {
        let mut highlighter = Highlighter::default();
        line(&mut highlighter, text, language)
    }

    fn token_of(parts: &[(String, Token)], needle: &str) -> Option<Token> {
        parts.iter().find(|(text, _)| text == needle).map(|(_, token)| *token)
    }

    /// 拼回去必须逐字等于原文：着色不许吞字或改字。
    #[test]
    fn tokens_reassemble_into_the_original_line() {
        for (text, language) in [
            (r#"let x = "hi"; // 注释"#, "rust"),
            ("def f(a):\n    return a", "python"),
            ("if [ -f x ]; then echo \"$x\"; fi", "bash"),
            ("{\"a\": [1, 2, null]}", "json"),
            ("SELECT * FROM t WHERE id = 1 -- x", "sql"),
            ("not a language at all", "brainfuck"),
        ] {
            for line in text.lines() {
                let joined: String = tokens(line, language).into_iter().map(|(t, _)| t).collect();
                assert_eq!(joined, line, "{language}");
            }
        }
    }

    #[test]
    fn keywords_strings_comments_and_numbers_get_distinct_tokens() {
        let parts = tokens(r#"let s = "x"; // hi"#, "rust");
        assert_eq!(token_of(&parts, "let"), Some(Token::Keyword));
        assert_eq!(token_of(&parts, "\"x\""), Some(Token::String));
        assert_eq!(token_of(&parts, "// hi"), Some(Token::Comment));

        let parts = tokens("fn main() { let n = 42; }", "rust");
        assert_eq!(token_of(&parts, "main"), Some(Token::Function));
        assert_eq!(token_of(&parts, "42"), Some(Token::Number));

        let parts = tokens("Foo::new()", "rust");
        assert_eq!(token_of(&parts, "Foo"), Some(Token::Type));
    }

    /// 跨行的块注释：第二行开头不该被当成代码。
    #[test]
    fn a_block_comment_keeps_its_state_across_lines() {
        let mut highlighter = Highlighter::default();
        let first = line(&mut highlighter, "/* start", "rust");
        assert!(
            first.iter().all(|(_, token)| *token == Token::Comment),
            "{first:?}"
        );
        assert_eq!(
            first.iter().map(|(text, _)| text.as_str()).collect::<String>(),
            "/* start"
        );
        let second = line(&mut highlighter, "still comment */ let x = 1;", "rust");
        // 注释部分（直到 `*/`）是一段，之后的 `let` 恢复关键字。
        let comment: String = second
            .iter()
            .take_while(|(_, token)| *token == Token::Comment)
            .map(|(text, _)| text.as_str())
            .collect();
        assert_eq!(comment, "still comment */", "{second:?}");
        assert_eq!(token_of(&second, "let"), Some(Token::Keyword));
        assert!(!highlighter.block_comment);
    }

    #[test]
    fn python_triple_quoted_strings_span_lines() {
        let mut highlighter = Highlighter::default();
        line(&mut highlighter, "x = \"\"\"doc", "python");
        let second = line(&mut highlighter, "more doc\"\"\"", "python");
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].1, Token::String);
    }

    /// 认不出的语言不着色：一行就是原文，一个 span。
    #[test]
    fn an_unknown_language_is_left_alone() {
        let parts = tokens("whatever // not a comment here", "brainfuck");
        assert_eq!(parts, vec![("whatever // not a comment here".to_string(), Token::Plain)]);
    }
}
