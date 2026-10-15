//! What the user can ask the app to do, from a click or from a shortcut.
//!
//! A clickable element records the action it stands for when it is drawn,
//! a shortcut maps to one in `shortcut`. `App::run` carries them out.

use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey, PhysicalKey};

use crate::core::files::Edition;
use crate::core::{ProjectId, SessionId};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Action {
    Minimize,
    Maximize,
    Quit,
    ToggleDiff(ProjectId),
    RemoveProject(ProjectId),
    Open(SessionId),
    /// Opens a session from one of the rows below its own. Not `Open`:
    /// a second click on the row of a session renames it, and these rows
    /// are not that row.
    OpenBelow(SessionId),
    CloseSession(SessionId),
    /// Opens the field and list for adding a project.
    AddProject,
    /// Opens the same field to start a session in a project: with an
    /// agent, or by continuing an earlier conversation.
    Launch(ProjectId),
    /// Shows the options of the agent in a row of that list.
    Options(usize),
    /// Takes the choice before the present one of the option in a row.
    Previous(usize),
    /// Chooses a row of that list.
    Pick(usize),
    /// Lists every earlier conversation: the row of that list for the
    /// rest. A hit of its own, so that the second click of a double click
    /// is not taken by the conversation that comes to stand in its place.
    More(usize),
    /// Switches that list from repositories to browsing folders.
    Browse,
    ClosePane(usize),
    /// Shows another session of the project beside the one in this pane,
    /// or starts one.
    Split(usize),
    /// Hides the message shown at the bottom of the sidebar.
    DismissNotice,
    /// Closes the focused one of two panes. The session keeps running.
    CloseSplit,
    /// Moves the keyboard to the other pane.
    FocusOther,
    /// Renames the session in front.
    Rename,
    Copy,
    Paste,
    /// A new session in the project being worked in, with the agent and
    /// the options that were used there last.
    NewDefault,
    /// The diff of the project being worked in.
    DiffActive,
    /// The list that starts a session, for the project being worked in.
    LaunchActive,
    /// The next or previous session.
    Cycle(isize),
    /// Font size one step up or down, or back to the configured size.
    Zoom(i8),
    /// Shows the column with the files of a project, or folds it away.
    ToggleTree,
    /// Folds the sidebar to a strip, or gives it its width back.
    ToggleSidebar,
    /// A click on the row of an entry there, by the number of the entry:
    /// opens or closes a folder, shows a file.
    TreeRow(u32),
    /// Shows the files of this project there.
    TreeOf(ProjectId),
    /// Opens the changes at the file that is shown and at this line of
    /// it, counted from one: a click on the number of a marked line. The
    /// text the number was drawn for says whether the line is still that
    /// line when the click is acted on.
    FileChanges(Edition, u32),
    /// Shows the file a row of the changes names: the row, and a number
    /// made of the name, to know that the row still names that file.
    DiffFile(u32, u32),
    /// Shows the file of Markdown that is shown as it is meant, or as
    /// its text again.
    TogglePreview,
}

/// The action a key press stands for. `base` is the key without modifiers
/// in lower case, `named` is set for keys that type nothing, `split` says
/// whether two panes are open.
///
/// Every shortcut needs Control, and all but the font size also Shift, so
/// nothing a terminal program uses is taken away from it.
pub fn shortcut(base: &str, named: Option<NamedKey>, mods: ModifiersState, split: bool) -> Option<Action> {
    if !mods.control_key() || mods.alt_key() {
        return None;
    }
    let shift = mods.shift_key();
    Some(match (shift, base, named) {
        (true, "c", _) => Action::Copy,
        (true, "v", _) => Action::Paste,
        (true, "t", _) => Action::NewDefault,
        (true, "d", _) => Action::DiffActive,
        (true, "o", _) => Action::AddProject,
        (true, "e", _) => Action::LaunchActive,
        (true, "r", _) => Action::Rename,
        (true, "b", _) => Action::ToggleSidebar,
        (true, "f", _) => Action::ToggleTree,
        (true, "m", _) => Action::TogglePreview,
        (true, "w", _) if split => Action::CloseSplit,
        (true, _, Some(NamedKey::ArrowLeft | NamedKey::ArrowRight)) if split => Action::FocusOther,
        (_, _, Some(NamedKey::Tab)) => Action::Cycle(if shift { -1 } else { 1 }),
        (_, "=" | "+", _) => Action::Zoom(1),
        (false, "-", _) => Action::Zoom(-1),
        (false, "0", _) => Action::Zoom(0),
        _ => return None,
    })
}

/// The key a shortcut is matched on: what the key types without modifiers,
/// in lower case. On a layout where that is not a Latin letter or digit
/// (Cyrillic, Greek, AZERTY's digit row) the position on the keyboard
/// decides, so Ctrl+Shift+C is the same physical key everywhere.
pub fn shortcut_key(base: &Key, physical: PhysicalKey) -> String {
    if let Key::Character(c) = base
        && c.is_ascii()
        && !c.is_empty()
    {
        return c.to_lowercase();
    }
    let PhysicalKey::Code(code) = physical else { return String::new() };
    match code {
        KeyCode::Equal => "=".into(),
        KeyCode::Minus => "-".into(),
        _ => {
            let name = format!("{code:?}");
            let last = name
                .strip_prefix("Key")
                .or_else(|| name.strip_prefix("Digit"))
                .filter(|rest| rest.len() == 1);
            last.map_or_else(String::new, str::to_lowercase)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcuts_need_control_and_leave_plain_keys_to_the_terminal() {
        let ctrl = ModifiersState::CONTROL;
        let ctrl_shift = ctrl | ModifiersState::SHIFT;
        assert_eq!(shortcut("c", None, ctrl_shift, false), Some(Action::Copy));
        assert_eq!(shortcut("b", None, ctrl_shift, false), Some(Action::ToggleSidebar));
        assert_eq!(shortcut("f", None, ctrl_shift, false), Some(Action::ToggleTree));
        assert_eq!(shortcut("m", None, ctrl_shift, false), Some(Action::TogglePreview));
        assert_eq!(shortcut("m", None, ctrl, false), None, "Ctrl+M is Enter to a program");
        assert_eq!(shortcut("f", None, ctrl, false), None, "Ctrl+F belongs to the program");
        assert_eq!(shortcut("c", None, ctrl, false), None, "Ctrl+C belongs to the program");
        assert_eq!(shortcut("c", None, ModifiersState::SHIFT, false), None);
        assert_eq!(shortcut("c", None, ctrl_shift | ModifiersState::ALT, false), None);
        assert_eq!(shortcut("", Some(NamedKey::Tab), ctrl, false), Some(Action::Cycle(1)));
        assert_eq!(shortcut("", Some(NamedKey::Tab), ctrl_shift, false), Some(Action::Cycle(-1)));
        assert_eq!(shortcut("+", None, ctrl_shift, false), Some(Action::Zoom(1)));
        assert_eq!(shortcut("0", None, ctrl, false), Some(Action::Zoom(0)));
        // Pane shortcuts exist only while there are two panes.
        assert_eq!(shortcut("w", None, ctrl_shift, false), None);
        assert_eq!(shortcut("w", None, ctrl_shift, true), Some(Action::CloseSplit));
        assert_eq!(shortcut("", Some(NamedKey::ArrowLeft), ctrl_shift, true), Some(Action::FocusOther));
        assert_eq!(shortcut("", Some(NamedKey::ArrowLeft), ctrl_shift, false), None);
    }

    #[test]
    fn shortcuts_fall_back_to_the_key_position_on_non_latin_layouts() {
        let key = |text: &str| Key::Character(text.into());
        let at = PhysicalKey::Code;
        assert_eq!(shortcut_key(&key("C"), at(KeyCode::KeyC)), "c");
        assert_eq!(shortcut_key(&key("с"), at(KeyCode::KeyC)), "c", "Cyrillic es on the C key");
        assert_eq!(shortcut_key(&key("à"), at(KeyCode::Digit0)), "0", "AZERTY digit row");
        assert_eq!(shortcut_key(&key("ψ"), at(KeyCode::KeyC)), "c");
        assert_eq!(shortcut_key(&Key::Named(NamedKey::Tab), at(KeyCode::Tab)), "");
        // German: the key right of 0 types ß and stands for its position.
        assert_eq!(shortcut_key(&key("ß"), at(KeyCode::Minus)), "-");
    }
}
