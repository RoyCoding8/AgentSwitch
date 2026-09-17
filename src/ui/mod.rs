pub mod chat_panel;
pub mod diff_panel;
pub mod editor_panel;
pub mod hooks_panel;
pub mod item_list;
pub mod sidebar;
pub mod status_bar;
pub mod theme;

pub fn tail_ellipsis(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let start = text
        .char_indices()
        .rev()
        .nth(max.saturating_sub(4))
        .map(|(i, _)| i)
        .unwrap_or(0);
    format!("...{}", &text[start..])
}
