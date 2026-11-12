//! Which app are you dictating into? Drives the writing style.

use crate::config::Config;
use crate::text::Style;

#[derive(Debug, Clone, Default)]
pub struct Target {
    pub app: String,
    pub title: String,
}

pub fn capture() -> Target {
    match active_win_pos_rs::get_active_window() {
        Ok(w) => {
            let stem = w.process_path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            let app = if w.app_name.trim().is_empty() { stem } else { format!("{} {}", w.app_name, stem) };
            Target { app: app.to_lowercase(), title: w.title.to_lowercase() }
        }
        Err(_) => Target::default(),
    }
}

const CASUAL: &[&str] = &["whatsapp", "telegram", "discord", "slack", "signal", "messenger", "teams", "instagram", "messages", "beeper"];
const EMAIL: &[&str] = &["outlook", "thunderbird", "mail", "spark", "superhuman", "mailspring"];
const CODE: &[&str] = &[
    "code", "cursor", "windsurf", "zed", "sublime", "idea", "pycharm", "webstorm", "rider", "clion", "goland",
    "android studio", "vim", "emacs", "terminal", "cmd", "powershell", "pwsh", "wezterm", "alacritty", "kitty",
    "iterm", "konsole", "warp", "ghostty", "xterm", "tilix", "mintty",
];
const CASUAL_TITLES: &[&str] = &["whatsapp", "discord", "slack", "telegram", "messenger", "instagram"];
const EMAIL_TITLES: &[&str] = &["gmail", "outlook", "inbox", "proton mail", "yahoo mail"];

pub fn style_for(t: &Target, cfg: &Config) -> Style {
    for (needle, style) in &cfg.apps {
        let n = needle.to_lowercase();
        if !n.is_empty() && (t.app.contains(&n) || t.title.contains(&n)) {
            if let Some(s) = Style::parse(style) {
                return s;
            }
        }
    }
    // Browser tabs: decide by title first.
    if CASUAL_TITLES.iter().any(|k| t.title.contains(k)) {
        return Style::Casual;
    }
    if EMAIL_TITLES.iter().any(|k| t.title.contains(k)) {
        return Style::Email;
    }
    if CASUAL.iter().any(|k| t.app.contains(k)) {
        return Style::Casual;
    }
    if EMAIL.iter().any(|k| t.app.contains(k)) {
        return Style::Email;
    }
    if CODE.iter().any(|k| t.app.split_whitespace().any(|w| w.contains(k))) {
        return Style::Code;
    }
    Style::Default
}

pub fn is_terminal(t: &Target) -> bool {
    ["terminal", "konsole", "alacritty", "kitty", "wezterm", "xterm", "tilix", "ghostty", "warp"]
        .iter()
        .any(|k| t.app.contains(k))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(app: &str, title: &str) -> Target {
        Target { app: app.into(), title: title.into() }
    }

    #[test]
    fn styles() {
        let c = Config::default();
        assert_eq!(style_for(&t("whatsapp whatsapp", ""), &c), Style::Casual);
        assert_eq!(style_for(&t("google chrome chrome", "inbox (3) - me@gmail.com - gmail"), &c), Style::Email);
        assert_eq!(style_for(&t("visual studio code code", "main.rs"), &c), Style::Code);
        assert_eq!(style_for(&t("windows terminal windowsterminal", ""), &c), Style::Code);
        assert_eq!(style_for(&t("notepad notepad", ""), &c), Style::Default);
    }
}
