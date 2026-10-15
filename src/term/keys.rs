//! Turns key, paste and mouse input into the bytes a terminal program expects.

use alacritty_terminal::term::TermMode;
use winit::keyboard::{Key, ModifiersState, NamedKey};

/// xterm modifier parameter: 1 + shift 1, alt 2, ctrl 4, super 8.
fn modifier(mods: ModifiersState) -> u8 {
    1 + mods.shift_key() as u8
        + 2 * mods.alt_key() as u8
        + 4 * mods.control_key() as u8
        + 8 * mods.super_key() as u8
}

/// Encodes a key press. `key` is the key with modifiers applied, `base` the
/// same key without them, `text` what the press types.
pub fn encode(
    key: &Key,
    base: &Key,
    text: Option<&str>,
    mods: ModifiersState,
    mode: TermMode,
) -> Option<Vec<u8>> {
    let m = modifier(mods);
    // Programs that enable the kitty keyboard protocol get unambiguous
    // `CSI code ; modifiers u` sequences for keys that are otherwise lossy.
    let kitty = mode.contains(TermMode::DISAMBIGUATE_ESC_CODES);
    let csi_u = |code: u32| format!("\x1b[{code};{m}u").into_bytes();
    let alt = |bytes: &[u8]| {
        let mut out = if mods.alt_key() { vec![0x1b] } else { Vec::new() };
        out.extend_from_slice(bytes);
        out
    };

    if let Key::Named(key) = key {
        // `CSI 1 ; m X` style keys.
        let letter = |c: char| {
            Some(if m == 1 {
                let intro =
                    if mode.contains(TermMode::APP_CURSOR) && "ABCDHF".contains(c) { 'O' } else { '[' };
                format!("\x1b{intro}{c}").into_bytes()
            } else {
                format!("\x1b[1;{m}{c}").into_bytes()
            })
        };
        // `CSI n ~` style keys.
        let tilde =
            |n: u8| Some(if m == 1 { format!("\x1b[{n}~") } else { format!("\x1b[{n};{m}~") }.into_bytes());
        return match key {
            NamedKey::ArrowUp => letter('A'),
            NamedKey::ArrowDown => letter('B'),
            NamedKey::ArrowRight => letter('C'),
            NamedKey::ArrowLeft => letter('D'),
            NamedKey::Home => letter('H'),
            NamedKey::End => letter('F'),
            NamedKey::Insert => tilde(2),
            NamedKey::Delete => tilde(3),
            NamedKey::PageUp => tilde(5),
            NamedKey::PageDown => tilde(6),
            NamedKey::F1 | NamedKey::F2 | NamedKey::F3 | NamedKey::F4 => {
                let c = match key {
                    NamedKey::F1 => 'P',
                    NamedKey::F2 => 'Q',
                    NamedKey::F3 => 'R',
                    _ => 'S',
                };
                Some(if m == 1 { format!("\x1bO{c}") } else { format!("\x1b[1;{m}{c}") }.into_bytes())
            }
            NamedKey::F5 => tilde(15),
            NamedKey::F6 => tilde(17),
            NamedKey::F7 => tilde(18),
            NamedKey::F8 => tilde(19),
            NamedKey::F9 => tilde(20),
            NamedKey::F10 => tilde(21),
            NamedKey::F11 => tilde(23),
            NamedKey::F12 => tilde(24),
            NamedKey::Enter if kitty && m != 1 => Some(csi_u(13)),
            NamedKey::Enter => Some(alt(b"\r")),
            NamedKey::Tab if mods.shift_key() && m == 2 => Some(b"\x1b[Z".to_vec()),
            NamedKey::Tab if kitty && m != 1 => Some(csi_u(9)),
            NamedKey::Tab => Some(alt(b"\t")),
            NamedKey::Backspace if kitty && m != 1 => Some(csi_u(127)),
            NamedKey::Backspace => Some(alt(if mods.control_key() { b"\x08" } else { b"\x7f" })),
            NamedKey::Escape if kitty => Some(if m == 1 { b"\x1b[27u".to_vec() } else { csi_u(27) }),
            NamedKey::Escape => Some(b"\x1b".to_vec()),
            NamedKey::Space if kitty && m > 2 => Some(csi_u(32)),
            NamedKey::Space if mods.control_key() => Some(alt(b"\0")),
            // A dead key followed by Space types the accent on its own: the
            // key is still Space, the text is the backtick or caret.
            NamedKey::Space => Some(alt(text.filter(|t| !t.is_empty()).unwrap_or(" ").as_bytes())),
            _ => None,
        };
    }

    if kitty
        && (mods.control_key() || mods.alt_key() || mods.super_key())
        && let Key::Character(base) = base
    {
        let c = base.chars().next()?.to_lowercase().next()?;
        return Some(csi_u(c as u32));
    }
    let text = text?;
    (!text.is_empty()).then(|| alt(text.as_bytes()))
}

/// Wraps pasted text the way the running program asked for.
pub fn paste(text: &str, mode: TermMode) -> Vec<u8> {
    if mode.contains(TermMode::BRACKETED_PASTE) {
        // The end marker inside the text would let a paste escape the
        // brackets, and some shells end a paste at Ctrl+C.
        // One pass into one buffer: a paste can be megabytes. The bytes
        // left out are ASCII and never part of a longer character.
        let mut out = Vec::with_capacity(text.len() + 12);
        out.extend_from_slice(b"\x1b[200~");
        out.extend(text.bytes().filter(|&b| b != 0x1b && b != 0x03));
        out.extend_from_slice(b"\x1b[201~");
        out
    } else {
        // Line ends become Return, and a Windows line end only one. What
        // counts is the Return in the text: the one written for the line
        // end before would take an empty line away.
        let mut out = Vec::with_capacity(text.len());
        let mut after_return = false;
        for b in text.bytes() {
            match b {
                b'\n' if after_return => {}
                b'\n' => out.push(b'\r'),
                _ => out.push(b),
            }
            after_return = b == b'\r';
        }
        out
    }
}

/// Encodes a mouse report. `button` is 0 to 2, 3 for a release without SGR,
/// 64 and 65 for the wheel, plus 32 while dragging. Cells are zero-based.
pub fn mouse(
    button: u8,
    pressed: bool,
    col: usize,
    row: usize,
    mods: ModifiersState,
    mode: TermMode,
) -> Option<Vec<u8>> {
    let code = button + 4 * mods.shift_key() as u8 + 8 * mods.alt_key() as u8 + 16 * mods.control_key() as u8;
    if mode.contains(TermMode::SGR_MOUSE) {
        let end = if pressed { 'M' } else { 'm' };
        Some(format!("\x1b[<{code};{};{}{end}", col + 1, row + 1).into_bytes())
    } else if col < 223 && row < 223 {
        let code = if pressed { code } else { code | 3 };
        Some(vec![0x1b, b'[', b'M', 32 + code, 33 + col as u8, 33 + row as u8])
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: ModifiersState = ModifiersState::empty();
    const SHIFT: ModifiersState = ModifiersState::SHIFT;
    const CTRL: ModifiersState = ModifiersState::CONTROL;
    const ALT: ModifiersState = ModifiersState::ALT;

    fn named(key: NamedKey, mods: ModifiersState, mode: TermMode) -> Option<String> {
        let key = Key::Named(key);
        encode(&key, &key, None, mods, mode).map(|b| String::from_utf8(b).unwrap())
    }

    fn typed(base: &str, text: &str, mods: ModifiersState, mode: TermMode) -> Option<String> {
        let key = Key::Character(text.into());
        encode(&key, &Key::Character(base.into()), Some(text), mods, mode)
            .map(|b| String::from_utf8(b).unwrap())
    }

    #[test]
    fn navigation_keys_follow_xterm() {
        let plain = TermMode::empty();
        assert_eq!(named(NamedKey::ArrowUp, NONE, plain).unwrap(), "\x1b[A");
        assert_eq!(named(NamedKey::ArrowUp, NONE, TermMode::APP_CURSOR).unwrap(), "\x1bOA");
        assert_eq!(named(NamedKey::ArrowRight, CTRL, TermMode::APP_CURSOR).unwrap(), "\x1b[1;5C");
        assert_eq!(named(NamedKey::End, SHIFT | CTRL, plain).unwrap(), "\x1b[1;6F");
        assert_eq!(named(NamedKey::Delete, NONE, plain).unwrap(), "\x1b[3~");
        assert_eq!(named(NamedKey::PageDown, ALT, plain).unwrap(), "\x1b[6;3~");
        assert_eq!(named(NamedKey::F1, NONE, plain).unwrap(), "\x1bOP");
        assert_eq!(named(NamedKey::F5, SHIFT, plain).unwrap(), "\x1b[15;2~");
        assert_eq!(named(NamedKey::Tab, SHIFT, plain).unwrap(), "\x1b[Z");
        assert_eq!(named(NamedKey::Backspace, NONE, plain).unwrap(), "\x7f");
        assert_eq!(named(NamedKey::Backspace, ALT, plain).unwrap(), "\x1b\x7f");
        assert_eq!(named(NamedKey::CapsLock, NONE, plain), None);
    }

    #[test]
    fn text_is_sent_as_typed_with_alt_as_escape_prefix() {
        let plain = TermMode::empty();
        assert_eq!(typed("a", "a", NONE, plain).unwrap(), "a");
        assert_eq!(typed("a", "A", SHIFT, plain).unwrap(), "A");
        assert_eq!(typed("c", "\x03", CTRL, plain).unwrap(), "\x03");
        assert_eq!(typed("b", "b", ALT, plain).unwrap(), "\x1bb");
        assert_eq!(typed("q", "@", NONE, plain).unwrap(), "@", "AltGr is not Alt");
    }

    #[test]
    fn the_kitty_protocol_disambiguates_modified_keys() {
        let kitty = TermMode::DISAMBIGUATE_ESC_CODES;
        assert_eq!(named(NamedKey::Enter, NONE, kitty).unwrap(), "\r");
        assert_eq!(named(NamedKey::Enter, SHIFT, kitty).unwrap(), "\x1b[13;2u");
        assert_eq!(named(NamedKey::Escape, NONE, kitty).unwrap(), "\x1b[27u");
        assert_eq!(named(NamedKey::Tab, CTRL, kitty).unwrap(), "\x1b[9;5u");
        assert_eq!(typed("c", "\x03", CTRL, kitty).unwrap(), "\x1b[99;5u");
        assert_eq!(typed("c", "C", CTRL | SHIFT, kitty).unwrap(), "\x1b[99;6u");
        assert_eq!(typed("a", "A", SHIFT, kitty).unwrap(), "A");
        assert_eq!(named(NamedKey::Enter, SHIFT, TermMode::empty()).unwrap(), "\r");
    }

    #[test]
    fn a_dead_key_followed_by_space_types_the_accent() {
        let space = Key::Named(NamedKey::Space);
        let typed = |text| encode(&space, &space, text, NONE, TermMode::empty()).unwrap();
        assert_eq!(typed(Some("`")), b"`");
        assert_eq!(typed(Some(" ")), b" ");
        assert_eq!(typed(None), b" ");
    }

    #[test]
    fn paste_is_bracketed_when_asked_and_cannot_break_out() {
        assert_eq!(paste("a\nb\r\nc", TermMode::empty()), b"a\rb\rc");
        // Empty lines stay, whichever line end the text has.
        assert_eq!(paste("a\n\nb\r\n\r\nc\n\n\n", TermMode::empty()), b"a\r\rb\r\rc\r\r\r");
        let hostile = "x\x1b[201~rm\x03 -rf";
        assert_eq!(paste(hostile, TermMode::BRACKETED_PASTE), b"\x1b[200~x[201~rm -rf\x1b[201~");
    }

    #[test]
    fn mouse_reports_use_sgr_or_the_legacy_form() {
        let sgr = TermMode::SGR_MOUSE;
        assert_eq!(mouse(0, true, 4, 9, NONE, sgr).unwrap(), b"\x1b[<0;5;10M");
        assert_eq!(mouse(0, false, 4, 9, NONE, sgr).unwrap(), b"\x1b[<0;5;10m");
        assert_eq!(mouse(64, true, 0, 0, CTRL, sgr).unwrap(), b"\x1b[<80;1;1M");
        assert_eq!(mouse(2, true, 0, 0, NONE, TermMode::empty()).unwrap(), [0x1b, b'[', b'M', 34, 33, 33]);
        assert_eq!(mouse(0, false, 1, 1, NONE, TermMode::empty()).unwrap(), [0x1b, b'[', b'M', 35, 34, 34]);
        assert_eq!(mouse(0, true, 300, 0, NONE, TermMode::empty()), None);
    }
}
