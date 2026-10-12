//! 路径解析与包含判断。
//!
//! 两个反复出错的点，集中在这里处理：
//!
//! 1. **相对路径的基准**。任务书里的 `workspace`/`write_paths` 相对**任务书文件**
//!    解析，配置里的相对**配置文件**解析。用当前工作目录做基准会让同一份夹具
//!    在不同目录下跑出不同授权。
//! 2. **包含判断**。授权检查靠前缀比较，所以必须在比较前把 `..`、`.` 与分隔符
//!    归一化；否则 `crates/../../etc` 会「看着像」在以 `crates/` 为前缀。

use std::path::{Component, Path, PathBuf};

use crate::error::{code, SpecError, SpecResult};

/// 相对 `base` 解析，并做纯词法归一化（不碰文件系统）。
///
/// 不做 `canonicalize`：授权解析必须能在目标路径还不存在时给出确定结果，
/// 而 `canonicalize` 会失败。符号链接由此**不被解析**，这一点写在
/// PRODUCT_PLAN §6.2 的未决项里，M2 再按真实沙箱能力收紧。
pub fn resolve_against(base: &Path, raw: &Path) -> PathBuf {
    let joined = if raw.is_absolute() { raw.to_path_buf() } else { base.join(raw) };
    normalize(&joined)
}

/// 把路径锚定成绝对路径（不碰文件系统）。
///
/// **为什么必须有这一步**：`WorkerConfig::load("tests/fixtures/c.json")` 拿到的
/// 是相对路径，它的 `parent()` 也是相对的 —— 拿这个相对父目录去解析配置里的
/// `../..`，得到的仍然是相对路径，于是「workspace 不是绝对路径」当场报错。
/// 测试一直用绝对路径，所以这条路一直没被走到；是照着文档跑那条命令才暴露的。
pub fn absolutize(path: &Path) -> PathBuf {
    if path.is_absolute() {
        return normalize(path);
    }
    match std::env::current_dir() {
        Ok(cwd) => normalize(&cwd.join(path)),
        // 取不到当前目录时保持原样：让调用方看到「仍是相对路径」这个结论，
        // 而不是在这里悄悄替它编一个基目录。
        Err(_) => normalize(path),
    }
}

/// 纯词法归一化：消掉 `.`，就地展开 `..`，统一分隔符。
pub fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                // 在根上再往上走就停在根；Windows 的前缀（C:\）也是根。
                if !out.pop() {
                    out.push(Component::ParentDir.as_os_str());
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() {
        out.push(".");
    }
    out
}

/// 比较用的归一化键：统一成 `/`，Windows 上再转小写。
///
/// Windows 的路径比较是大小写不敏感的（旧 C# 侧用 `OrdinalIgnoreCase`），
/// 这里保持一致，免得 `C:/Work` 与 `c:/work` 被当成两个工作区。
pub fn comparison_key(path: &Path) -> String {
    let s = normalize(path).to_string_lossy().replace('\\', "/");
    if cfg!(windows) { s.to_lowercase() } else { s }
}

/// 两个路径是否指向同一个位置。
pub fn same_path(a: &Path, b: &Path) -> bool {
    comparison_key(a) == comparison_key(b)
}

/// `candidate` 是否落在 `allowed` 之下（含自身）。
///
/// 两边都假定已相对 workspace 解析为绝对路径。
pub fn is_within(candidate: &Path, allowed: &Path) -> bool {
    let c = comparison_key(candidate);
    let a = comparison_key(allowed);
    if c == a {
        return true;
    }
    let mut prefix = a.clone();
    if !prefix.ends_with('/') {
        prefix.push('/');
    }
    c.starts_with(&prefix)
}

/// 校验一个「相对 workspace 的请求路径」：不允许逃出 workspace。
///
/// 返回相对 workspace 的归一化路径。`..` 逃逸报 [`code::AUTH_EXCEEDED`]：
/// 这属于越权，不是格式错误，脚本该按越权处理。
pub fn workspace_relative(workspace: &Path, raw: &Path) -> SpecResult<PathBuf> {
    let abs = resolve_against(workspace, raw);
    if !is_within(&abs, workspace) {
        return Err(SpecError::new(
            code::AUTH_EXCEEDED,
            format!("路径 `{}` 逃出了 workspace `{}`", raw.display(), workspace.display()),
        ));
    }
    let rel = abs
        .strip_prefix(&normalize(workspace))
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|_| PathBuf::from("."));
    if rel.as_os_str().is_empty() {
        Ok(PathBuf::from("."))
    } else {
        Ok(rel)
    }
}

#[cfg(test)]
pub(crate) fn test_abs(p: &str) -> PathBuf {
    PathBuf::from(test_abs_str(p))
}

/// 测试用的绝对路径字符串，统一用正斜杠 —— 它还要被塞进 JSON 字面量，
/// 反斜杠在那里要转义，会让测试读起来全是 `\\\\`。
#[cfg(test)]
pub(crate) fn test_abs_str(p: &str) -> String {
    let trimmed = p.trim_start_matches(['/', '\\']);
    if cfg!(windows) {
        format!("C:/{trimmed}")
    } else {
        format!("/{trimmed}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_removes_curdir_and_parentdir() {
        assert_eq!(normalize(Path::new("a/./b/../c")), PathBuf::from("a/c"));
        assert_eq!(normalize(Path::new("./a")), PathBuf::from("a"));
    }

    #[test]
    fn within_uses_component_boundaries() {
        let base = Path::new("/work/crates");
        assert!(is_within(Path::new("/work/crates/x.rs"), base));
        assert!(is_within(Path::new("/work/crates"), base));
        // 关键负例：`crates-extra` 不是 `crates` 的子路径。
        assert!(!is_within(Path::new("/work/crates-extra/x.rs"), base));
        assert!(!is_within(Path::new("/work/src/x.rs"), base));
    }

    #[test]
    fn workspace_relative_rejects_escape() {
        let ws = Path::new("/work");
        assert_eq!(workspace_relative(ws, Path::new("crates/x")).unwrap(), PathBuf::from("crates/x"));
        let err = workspace_relative(ws, Path::new("crates/../../etc/passwd")).unwrap_err();
        assert!(err.is(code::AUTH_EXCEEDED), "{err}");
    }

    #[test]
    fn relative_origin_is_anchored_before_resolving() {
        // 回归测试：`WorkerConfig::load("tests/fixtures/c.json")` 拿到的父目录也是
        // 相对的，拿它去解析配置里的 `../..` 会得到一个相对路径 —— 然后在
        // 「workspace 必须是绝对路径」那里炸掉。必须先把基准锚成绝对路径。
        let config_path = Path::new("tests/fixtures/worker-offline.json");
        let anchored = absolutize(config_path);
        assert!(anchored.is_absolute(), "锚定后仍是相对路径：{anchored:?}");

        let origin = anchored.parent().expect("有父目录");
        let workspace = resolve_against(origin, Path::new("../.."));
        assert!(workspace.is_absolute(), "解析后仍是相对路径：{workspace:?}");
        assert_eq!(workspace, absolutize(Path::new(".")));
    }

    #[test]
    fn absolutize_keeps_absolute_paths_intact() {
        let absolute = test_abs("work/project");
        assert_eq!(absolutize(&absolute), absolute);
    }

    #[cfg(windows)]
    #[test]
    fn windows_comparison_is_case_insensitive() {
        assert!(same_path(Path::new("C:/Work/A"), Path::new("c:/work/a")));
    }
}
