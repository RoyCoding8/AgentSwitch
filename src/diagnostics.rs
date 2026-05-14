use crate::{scanner, types::*};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone)]
pub struct DiffSide {
    pub state: ItemState,
    pub path: PathBuf,
    pub fingerprint: String,
    pub summary: String,
    pub warnings: Vec<String>,
    pub count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffStatus {
    Same,
    ProjectOnly,
    GlobalOnly,
    Differs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffFilter {
    All,
    OnlyDifferences,
    ProjectOnly,
    GlobalOnly,
    Conflicts,
}

#[derive(Debug, Clone)]
pub struct DiffRow {
    pub kind: ItemKind,
    pub name: String,
    pub project: Option<DiffSide>,
    pub global: Option<DiffSide>,
    pub status: DiffStatus,
    pub warnings: Vec<String>,
    pub notes: Vec<String>,
}

impl DiffStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Same => "Same",
            Self::ProjectOnly => "Project",
            Self::GlobalOnly => "Global",
            Self::Differs => "Differs",
        }
    }
}

impl DiffFilter {
    pub const ALL: &[DiffFilter] = &[
        Self::All,
        Self::OnlyDifferences,
        Self::ProjectOnly,
        Self::GlobalOnly,
        Self::Conflicts,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::OnlyDifferences => "Only differences",
            Self::ProjectOnly => "Project only",
            Self::GlobalOnly => "Global only",
            Self::Conflicts => "Conflicts",
        }
    }

    pub fn matches(self, row: &DiffRow) -> bool {
        match self {
            Self::All => true,
            Self::OnlyDifferences => row.status != DiffStatus::Same || row.has_conflict(),
            Self::ProjectOnly => row.status == DiffStatus::ProjectOnly,
            Self::GlobalOnly => row.status == DiffStatus::GlobalOnly,
            Self::Conflicts => row.has_conflict(),
        }
    }
}

impl DiffRow {
    pub fn has_conflict(&self) -> bool {
        !self.warnings.is_empty()
    }
}

pub fn default_filter(rows: &[DiffRow]) -> DiffFilter {
    if rows
        .iter()
        .any(|r| r.status != DiffStatus::Same || r.has_conflict())
    {
        DiffFilter::OnlyDifferences
    } else {
        DiffFilter::All
    }
}

pub fn build(provider: ProviderId, workspace: &Path) -> Vec<DiffRow> {
    let project = scanner::scan_provider(provider, workspace, Scope::Project);
    let global = scanner::scan_provider(provider, workspace, Scope::Global);
    build_rows(&project, &global)
}

fn build_rows(project: &[ConfigItem], global: &[ConfigItem]) -> Vec<DiffRow> {
    let project = collect(project, Scope::Project);
    let global = collect(global, Scope::Global);
    let mut keys: Vec<_> = project.keys().chain(global.keys()).cloned().collect();
    keys.sort();
    keys.dedup();
    keys.into_iter()
        .map(|key| {
            let p = project.get(&key).cloned();
            let g = global.get(&key).cloned();
            let status = match (&p, &g) {
                (Some(a), Some(b)) if a.state == b.state && a.fingerprint == b.fingerprint => {
                    DiffStatus::Same
                }
                (Some(_), Some(_)) => DiffStatus::Differs,
                (Some(_), None) => DiffStatus::ProjectOnly,
                (None, Some(_)) => DiffStatus::GlobalOnly,
                _ => DiffStatus::Same,
            };
            let mut warnings = side_warnings(&p);
            warnings.extend(side_warnings(&g));
            if status == DiffStatus::Differs && matches!(key.0, ItemKind::Hook | ItemKind::Mcp) {
                warnings.push(format!(
                    "{} differs between project and global",
                    key.0.label()
                ));
            }
            let notes = if p.is_some() && g.is_some() {
                vec!["Project value takes precedence over global".into()]
            } else {
                vec![]
            };
            DiffRow {
                kind: key.0,
                name: key.1,
                project: p,
                global: g,
                status,
                warnings,
                notes,
            }
        })
        .collect()
}

fn collect(items: &[ConfigItem], scope: Scope) -> BTreeMap<(ItemKind, String), DiffSide> {
    let mut grouped: BTreeMap<(ItemKind, String), Vec<DiffSide>> = BTreeMap::new();
    for item in items {
        grouped
            .entry((item.kind, item_key(item)))
            .or_default()
            .push(side_from_item(item));
    }
    grouped
        .into_iter()
        .map(|(key, sides)| {
            let kind = key.0;
            (key, combine(scope, kind, sides))
        })
        .collect()
}

fn combine(scope: Scope, kind: ItemKind, mut sides: Vec<DiffSide>) -> DiffSide {
    if sides.len() == 1 {
        return sides.remove(0);
    }
    sides.sort_by(|a, b| a.fingerprint.cmp(&b.fingerprint));
    let count = sides.len();
    let mut first = sides.remove(0);
    first.state = if sides.iter().any(|s| s.state.is_enabled()) || first.state.is_enabled() {
        ItemState::Enabled
    } else {
        ItemState::Disabled
    };
    first.fingerprint = std::iter::once(first.fingerprint.clone())
        .chain(sides.iter().map(|s| s.fingerprint.clone()))
        .collect::<Vec<_>>()
        .join("|");
    first.summary = format!("{count} entries · {}", first.summary);
    first.warnings.push(format!(
        "Duplicate {} {} entries",
        scope_label(scope),
        kind.label()
    ));
    first.count = count;
    first
}

fn side_from_item(item: &ConfigItem) -> DiffSide {
    let raw = item.detail.as_deref().unwrap_or("");
    let value = parse_detail(raw);
    DiffSide {
        state: item.state,
        path: item.path.clone(),
        fingerprint: fingerprint(&value),
        summary: summary_for(item, &value),
        warnings: warnings_for(item, &value),
        count: 1,
    }
}

fn item_key(item: &ConfigItem) -> String {
    if let Some(loc) = &item.hook_loc {
        return format!(
            "{}/{}",
            loc.event.trim_start_matches("_stashed_"),
            loc.hook_name
        );
    }
    item.name.clone()
}

fn parse_detail(raw: &str) -> Value {
    serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.into()))
}

fn fingerprint(value: &Value) -> String {
    serde_json::to_string(&canonical(value, None, true)).unwrap_or_else(|_| value.to_string())
}

fn canonical(value: &Value, key: Option<&str>, compare: bool) -> Value {
    if compare && key.map(is_secret_key).unwrap_or(false) {
        return secret_shape(value);
    }
    match value {
        Value::Array(arr) => {
            Value::Array(arr.iter().map(|v| canonical(v, None, compare)).collect())
        }
        Value::Object(obj) => {
            let mut sorted = serde_json::Map::new();
            let mut keys: Vec<_> = obj.keys().collect();
            keys.sort();
            for k in keys {
                sorted.insert(k.clone(), canonical(&obj[k], Some(k), compare));
            }
            Value::Object(sorted)
        }
        _ => value.clone(),
    }
}

fn secret_shape(value: &Value) -> Value {
    match value {
        Value::Array(arr) => Value::Array(arr.iter().map(secret_shape).collect()),
        Value::Object(obj) => {
            let mut sorted = serde_json::Map::new();
            let mut keys: Vec<_> = obj.keys().collect();
            keys.sort();
            for k in keys {
                sorted.insert(k.clone(), secret_shape(&obj[k]));
            }
            Value::Object(sorted)
        }
        Value::String(_) => Value::String("<secret:string>".into()),
        Value::Number(_) => Value::String("<secret:number>".into()),
        Value::Bool(_) => Value::String("<secret:bool>".into()),
        Value::Null => Value::Null,
    }
}

fn summary_for(item: &ConfigItem, value: &Value) -> String {
    match item.kind {
        ItemKind::Mcp => mcp_summary(value),
        ItemKind::Hook => hook_summary(item, value),
        _ => {
            let state = if item.state.is_enabled() { "on" } else { "off" };
            let file = item.path.file_name().unwrap_or_default().to_string_lossy();
            format!("{state} · {file}")
        }
    }
}

fn mcp_summary(value: &Value) -> String {
    let command = str_field(value, &["command", "cmd"]);
    let url = str_field(value, &["url", "endpoint"]);
    let transport = str_field(value, &["transport", "type"])
        .map(sanitize_text)
        .unwrap_or_else(|| {
            if command.is_some() {
                "stdio".into()
            } else if url.is_some() {
                "http".into()
            } else {
                "unknown".into()
            }
        });
    let target = command
        .map(|s| format!("cmd {}", sanitize_text(s)))
        .or_else(|| url.map(|s| format!("url {}", sanitize_text(s))))
        .unwrap_or_else(|| "missing command/url".into());
    let env_count = object_len(value, "env").unwrap_or(0);
    format!("{transport} · {target} · env {env_count}")
}

fn hook_summary(item: &ConfigItem, value: &Value) -> String {
    let event = item
        .hook_loc
        .as_ref()
        .map(|l| l.event.trim_start_matches("_stashed_"))
        .unwrap_or("hook");
    let matcher = str_field(value, &["matcher"]).unwrap_or("*");
    let handler = hook_handler(value)
        .or_else(|| item.hook_loc.as_ref().map(|l| l.hook_name.as_str()))
        .unwrap_or(&item.name);
    format!("{event} · {matcher} · {}", sanitize_text(handler))
}

fn warnings_for(item: &ConfigItem, value: &Value) -> Vec<String> {
    let mut out = vec![];
    if item.kind == ItemKind::Mcp
        && str_field(value, &["command", "cmd"]).is_none()
        && str_field(value, &["url", "endpoint"]).is_none()
    {
        out.push("MCP server is missing command/url".into());
    }
    out
}

fn side_warnings(side: &Option<DiffSide>) -> Vec<String> {
    side.as_ref()
        .map(|s| s.warnings.clone())
        .unwrap_or_default()
}

fn hook_handler(value: &Value) -> Option<&str> {
    value
        .get("hooks")
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .and_then(|h| str_field(h, &["name", "command", "cmd"]))
        .or_else(|| str_field(value, &["name", "command", "cmd"]))
}

fn str_field<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|k| value.get(*k).and_then(|v| v.as_str()))
}

fn object_len(value: &Value, key: &str) -> Option<usize> {
    value.get(key).and_then(|v| v.as_object()).map(|o| o.len())
}

fn sanitize_text(text: &str) -> String {
    let lower = text.to_ascii_lowercase();
    if lower.contains("token=") || lower.contains("key=") || lower.contains("secret=") {
        return text.split('?').next().unwrap_or(text).to_string() + "?<redacted>";
    }
    if text.len() > 56 {
        format!("{}...", &text[..53])
    } else {
        text.into()
    }
}

fn is_secret_key(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    [
        "token",
        "secret",
        "password",
        "passwd",
        "apikey",
        "api_key",
        "authorization",
        "credential",
    ]
    .iter()
    .any(|s| k.contains(s))
}

fn scope_label(scope: Scope) -> &'static str {
    match scope {
        Scope::Project => "project",
        Scope::Global => "global",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(name: &str, kind: ItemKind, state: ItemState, detail: &str) -> ConfigItem {
        let mut item = ConfigItem::new(
            name,
            kind,
            PathBuf::from(format!("{name}.json")),
            ProviderId::Codex,
        );
        item.state = state;
        item.detail = Some(detail.into());
        item
    }

    fn hook(name: &str, detail: &str) -> ConfigItem {
        let mut item = item(name, ItemKind::Hook, ItemState::Enabled, detail);
        item.hook_loc = Some(HookLoc {
            event: "PreToolUse".into(),
            index: 0,
            hook_name: name.into(),
        });
        item
    }

    #[test]
    fn classifies_project_global_rows() {
        let project = vec![
            item(
                "same",
                ItemKind::Mcp,
                ItemState::Enabled,
                r#"{"command":"a"}"#,
            ),
            item(
                "diff",
                ItemKind::Mcp,
                ItemState::Enabled,
                r#"{"command":"a"}"#,
            ),
            item(
                "local",
                ItemKind::Hook,
                ItemState::Enabled,
                r#"{"matcher":"*"}"#,
            ),
        ];
        let global = vec![
            item(
                "same",
                ItemKind::Mcp,
                ItemState::Enabled,
                r#"{"command":"a"}"#,
            ),
            item(
                "diff",
                ItemKind::Mcp,
                ItemState::Disabled,
                r#"{"command":"a"}"#,
            ),
            item(
                "home",
                ItemKind::Hook,
                ItemState::Enabled,
                r#"{"matcher":"*"}"#,
            ),
        ];
        let rows = build_rows(&project, &global);
        let status = |name: &str| rows.iter().find(|r| r.name == name).unwrap().status;
        assert_eq!(status("same"), DiffStatus::Same);
        assert_eq!(status("diff"), DiffStatus::Differs);
        assert_eq!(status("local"), DiffStatus::ProjectOnly);
        assert_eq!(status("home"), DiffStatus::GlobalOnly);
    }

    #[test]
    fn canonical_json_ignores_key_order() {
        let project = vec![item(
            "srv",
            ItemKind::Mcp,
            ItemState::Enabled,
            r#"{"env":{"B":"2","A":"1"},"command":"node"}"#,
        )];
        let global = vec![item(
            "srv",
            ItemKind::Mcp,
            ItemState::Enabled,
            r#"{"command":"node","env":{"A":"1","B":"2"}}"#,
        )];
        assert_eq!(build_rows(&project, &global)[0].status, DiffStatus::Same);
    }

    #[test]
    fn secret_values_are_redacted_for_compare_and_display() {
        let project = vec![item(
            "srv",
            ItemKind::Mcp,
            ItemState::Enabled,
            r#"{"command":"x","env":{"API_KEY":"one"}}"#,
        )];
        let global = vec![item(
            "srv",
            ItemKind::Mcp,
            ItemState::Enabled,
            r#"{"command":"x","env":{"API_KEY":"two"}}"#,
        )];
        let row = &build_rows(&project, &global)[0];
        assert_eq!(row.status, DiffStatus::Same);
        assert!(!row.project.as_ref().unwrap().summary.contains("one"));
        assert!(!row.global.as_ref().unwrap().summary.contains("two"));
    }

    #[test]
    fn summarizes_mcp_and_flags_missing_target() {
        let ok = item(
            "srv",
            ItemKind::Mcp,
            ItemState::Enabled,
            r#"{"command":"node","env":{"A":"1","B":"2"}}"#,
        );
        let bad = item(
            "bad",
            ItemKind::Mcp,
            ItemState::Enabled,
            r#"{"env":{"A":"1"}}"#,
        );
        let rows = build_rows(&[ok, bad], &[]);
        let srv = rows.iter().find(|r| r.name == "srv").unwrap();
        let bad = rows.iter().find(|r| r.name == "bad").unwrap();
        assert!(srv.project.as_ref().unwrap().summary.contains("stdio"));
        assert!(srv.project.as_ref().unwrap().summary.contains("env 2"));
        assert!(bad.has_conflict());
    }

    #[test]
    fn detects_duplicate_hooks_and_filters_conflicts() {
        let rows = build_rows(
            &[
                hook("fmt", r#"{"matcher":"Edit"}"#),
                hook("fmt", r#"{"matcher":"Edit"}"#),
            ],
            &[],
        );
        assert!(rows[0].has_conflict());
        assert!(DiffFilter::Conflicts.matches(&rows[0]));
        assert!(DiffFilter::OnlyDifferences.matches(&rows[0]));
    }
}
