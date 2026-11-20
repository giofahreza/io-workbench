//! Nerd Font icons for the interface. Every one is drawn followed by a space, since
//! in a non-monospaced Nerd Font variant an icon can be wider than its cell. Without
//! a Nerd Font in the terminal they show as empty boxes.

use crate::activity::Class;

/// A pinned chat, and a pinned prompt.
pub const PIN: &str = "\u{f08d}";
pub const FOLDER: &str = "\u{f07b}";
pub const FOLDER_OPEN: &str = "\u{f07c}";
/// A chat, and the one that is open.
pub const CHAT: &str = "\u{f0e5}";
pub const CHAT_CURRENT: &str = "\u{f075}";
/// More sessions below, and older messages above.
pub const MORE: &str = "\u{f103}";
pub const OLDER: &str = "\u{f102}";
pub const CONNECTED: &str = "\u{f0c1}";
pub const DISCONNECTED: &str = "\u{f127}";

const TERMINAL: &str = "\u{f120}";
const FILE: &str = "\u{f15c}";
const PENCIL: &str = "\u{f040}";
const SEARCH: &str = "\u{f002}";
const GLOBE: &str = "\u{f0ac}";
const ROBOT: &str = "\u{f06a9}";
const TASKS: &str = "\u{f046}";
const WRENCH: &str = "\u{f0ad}";

/// The icon for a kind of tool.
pub fn tool(class: Class) -> &'static str {
    match class {
        Class::Exec => TERMINAL,
        Class::Read => FILE,
        Class::Edit => PENCIL,
        Class::Search => SEARCH,
        Class::Web => GLOBE,
        Class::Agent => ROBOT,
        Class::Todo => TASKS,
        Class::Other => WRENCH,
    }
}
