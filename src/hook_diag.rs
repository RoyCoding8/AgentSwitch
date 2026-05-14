use crate::{scanner, types::*};
use serde_json::Value;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookFilter {
    All,
    Enabled,
    Disabled,
    BlockCapable,
    Warnings,
    Project,
    Global,
}

#[derive(Debug, Clone)]
pub struct HookRow {
    pub provider: ProviderId,
    pub scope: Scope,
    pub state: ItemState,
    pub path: PathBuf,
    pub event: String,
    pub matcher: String,
    pub handler: String,
    pub handler_kind: String,
    pub behavior: String,
    pub execution: String,
    pub timeout: Option<u64>,
    pub order: usize,
    pub warnings: Vec<String>,
}

impl HookFilter {
    pub const ALL: &[HookFilter] = &[
        Self::All,
        Self::Enabled,
        Self::Disabled,
        Self::BlockCapable,
        Self::Warnings,
        Self::Project,
        Self::Global,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Enabled => "Enabled",
            Self::Disabled => "Disabled",
            Self::BlockCapable => "Block capable",
            Self::Warnings => "Warnings",
            Self::Project => "Project",
            Self::Global => "Global",
        }
    }
    pub fn matches(self, row: &HookRow) -> bool {
        match self {
            Self::All => true,
            Self::Enabled => row.state.is_enabled(),
            Self::Disabled => !row.state.is_enabled(),
            Self::BlockCapable => row.behavior.contains("block"),
            Self::Warnings => !row.warnings.is_empty(),
            Self::Project => row.scope == Scope::Project,
            Self::Global => row.scope == Scope::Global,
        }
    }
}

pub fn default_filter(rows: &[HookRow]) -> HookFilter {
    if rows.iter().any(|r| !r.warnings.is_empty()) {
        HookFilter::Warnings
    } else {
        HookFilter::All
    }
}

pub fn build(provider: ProviderId, workspace: &Path) -> Vec<HookRow> {
    let mut rows = vec![];
    for scope in [Scope::Project, Scope::Global] {
        let root = if scope == Scope::Project {
            workspace.to_owned()
        } else {
            dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
        };
        rows.extend(
            scanner::scan_provider(provider, &root, scope)
                .into_iter()
                .filter(|i| i.kind == ItemKind::Hook)
                .map(|i| row(i, scope)),
        );
    }
    add_group_warnings(&mut rows);
    rows.sort_by(|a, b| {
        (a.event.as_str(), a.scope_key(), a.order, a.handler.as_str()).cmp(&(
            b.event.as_str(),
            b.scope_key(),
            b.order,
            b.handler.as_str(),
        ))
    });
    rows
}

fn row(item: ConfigItem, scope: Scope) -> HookRow {
    let value = parse(item.detail.as_deref().unwrap_or(""));
    let loc = item.hook_loc.clone().unwrap_or(HookLoc {
        event: "hook".into(),
        index: 0,
        hook_name: item.name.clone(),
    });
    let event = loc.event.trim_start_matches("_stashed_").to_string();
    let matcher = str_field(&value, &["matcher"]).unwrap_or("*").to_string();
    let (handler_kind, handler) =
        handler(&value).unwrap_or_else(|| ("unknown".into(), loc.hook_name.clone()));
    let timeout = timeout(&value);
    let mut row = HookRow {
        provider: item.provider,
        scope,
        state: item.state,
        path: item.path,
        event,
        matcher,
        handler,
        handler_kind,
        behavior: behavior(item.provider, &loc.event),
        execution: execution(item.provider, &loc.event),
        timeout,
        order: loc.index + 1,
        warnings: vec![],
    };
    row.warnings = own_warnings(&row);
    row
}

fn add_group_warnings(rows: &mut [HookRow]) {
    let mut same_scope: HashMap<(ProviderId, Scope, String, String, String), Vec<usize>> =
        HashMap::new();
    let mut cross_scope: HashMap<(ProviderId, String, String), Vec<usize>> = HashMap::new();
    for (i, r) in rows.iter().enumerate() {
        same_scope
            .entry((
                r.provider,
                r.scope,
                r.event.clone(),
                r.matcher.clone(),
                r.handler.clone(),
            ))
            .or_default()
            .push(i);
        cross_scope
            .entry((r.provider, r.event.clone(), r.matcher.clone()))
            .or_default()
            .push(i);
    }
    for ids in same_scope.values().filter(|ids| ids.len() > 1) {
        for &i in ids {
            rows[i].warnings.push("Duplicate hook in same scope".into());
        }
    }
    for ids in cross_scope.values() {
        let has_project = ids.iter().any(|&i| rows[i].scope == Scope::Project);
        let has_global = ids.iter().any(|&i| rows[i].scope == Scope::Global);
        if has_project && has_global {
            let mut handlers: Vec<_> = ids.iter().map(|&i| rows[i].handler.clone()).collect();
            handlers.sort();
            handlers.dedup();
            for &i in ids {
                rows[i].warnings.push("Global/project hook overlap".into());
                if handlers.len() > 1 {
                    rows[i]
                        .warnings
                        .push("Same event+matcher has different handlers".into());
                }
            }
        }
    }
    for r in rows {
        r.warnings.sort();
        r.warnings.dedup();
    }
}

fn own_warnings(r: &HookRow) -> Vec<String> {
    let mut out = vec![];
    if r.handler_kind == "unknown" || r.handler.trim().is_empty() {
        out.push("Missing handler".into());
    }
    if r.behavior.contains("block") && (r.matcher == "*" || r.matcher.is_empty()) {
        out.push("Broad matcher on block-capable hook".into());
    }
    if r.timeout.is_none() && (r.event.contains("Start") || r.event.contains("Stop")) {
        out.push("Missing timeout on lifecycle hook".into());
    }
    if r.timeout.unwrap_or(0) > 120 {
        out.push("Timeout over 120s".into());
    }
    out
}

fn behavior(provider: ProviderId, event: &str) -> String {
    if provider == ProviderId::OpenCode {
        return "plugin".into();
    }
    if event.contains("PreToolUse") || event.contains("UserPromptSubmit") || event.contains("Stop")
    {
        "block-capable".into()
    } else if event.contains("PostToolUse") {
        "warning/feedback".into()
    } else if event.contains("SessionStart") {
        "context-injecting".into()
    } else if event.contains("Notification") {
        "notification".into()
    } else if event.contains("PreCompact") {
        "cleanup".into()
    } else {
        "unknown".into()
    }
}

fn execution(provider: ProviderId, event: &str) -> String {
    if provider == ProviderId::OpenCode {
        "plugin load order".into()
    } else if event.contains("Stop") || event.contains("SessionStart") {
        "ordered, may block".into()
    } else {
        "configured order".into()
    }
}

fn handler(value: &Value) -> Option<(String, String)> {
    if let Some(s) = value.as_str() {
        return Some(("plugin".into(), s.into()));
    }
    let h = value
        .get("hooks")
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .unwrap_or(value);
    for (kind, keys) in [
        ("command", &["command", "cmd"][..]),
        ("url", &["url", "endpoint"][..]),
        ("name", &["name"][..]),
    ] {
        if let Some(v) = str_field(h, keys) {
            return Some((kind.into(), clean(v)));
        }
    }
    None
}

fn timeout(value: &Value) -> Option<u64> {
    let h = value
        .get("hooks")
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .unwrap_or(value);
    num_field(h, &["timeout", "timeout_ms", "timeoutMs"])
        .or_else(|| num_field(value, &["timeout", "timeout_ms", "timeoutMs"]))
}

fn parse(raw: &str) -> Value {
    serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.into()))
}
fn str_field<'a>(v: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|k| v.get(*k).and_then(|v| v.as_str()))
}
fn num_field(v: &Value, keys: &[&str]) -> Option<u64> {
    keys.iter().find_map(|k| v.get(*k).and_then(|v| v.as_u64()))
}
fn clean(s: &str) -> String {
    if s.len() > 70 {
        format!("{}...", &s[..67])
    } else {
        s.into()
    }
}

impl HookRow {
    fn scope_key(&self) -> u8 {
        if self.scope == Scope::Project {
            0
        } else {
            1
        }
    }
    pub fn scope_label(&self) -> &'static str {
        if self.scope == Scope::Project {
            "Project"
        } else {
            "Global"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(name: &str, detail: &str, scope: Scope) -> HookRow {
        let mut i = ConfigItem::new(
            name,
            ItemKind::Hook,
            PathBuf::from("hooks.json"),
            ProviderId::Claude,
        );
        i.detail = Some(detail.into());
        i.hook_loc = Some(HookLoc {
            event: "PreToolUse".into(),
            index: 0,
            hook_name: name.into(),
        });
        row(i, scope)
    }

    #[test]
    fn classifies_handler_timeout_and_blocking() {
        let r = item(
            "lint",
            r#"{"matcher":"Edit","hooks":[{"command":"lint","timeout":30}]}"#,
            Scope::Project,
        );
        assert_eq!(r.handler_kind, "command");
        assert_eq!(r.handler, "lint");
        assert_eq!(r.timeout, Some(30));
        assert!(r.behavior.contains("block"));
    }

    #[test]
    fn warns_for_broad_blocking_missing_handler() {
        let r = item("bad", r#"{"matcher":"*"}"#, Scope::Project);
        assert!(r.warnings.iter().any(|w| w.contains("Broad matcher")));
        assert!(r.warnings.iter().any(|w| w.contains("Missing handler")));
    }

    #[test]
    fn detects_duplicates_and_project_global_overlap() {
        let mut rows = vec![
            item(
                "a",
                r#"{"matcher":"Edit","hooks":[{"command":"x"}]}"#,
                Scope::Project,
            ),
            item(
                "a",
                r#"{"matcher":"Edit","hooks":[{"command":"x"}]}"#,
                Scope::Project,
            ),
            item(
                "a",
                r#"{"matcher":"Edit","hooks":[{"command":"y"}]}"#,
                Scope::Global,
            ),
        ];
        add_group_warnings(&mut rows);
        assert!(rows[0].warnings.iter().any(|w| w.contains("Duplicate")));
        assert!(rows[2]
            .warnings
            .iter()
            .any(|w| w.contains("different handlers")));
    }

    #[test]
    fn filters_warning_rows() {
        let r = item("bad", r#"{"matcher":"*"}"#, Scope::Project);
        assert!(HookFilter::Warnings.matches(&r));
        assert!(HookFilter::Project.matches(&r));
    }
}
