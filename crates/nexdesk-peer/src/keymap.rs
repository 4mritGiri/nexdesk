//! Physical key -> Linux evdev code, the key numbering used on the wire (`Msg::Key`).
//! Linux gets the code from the windowing system itself (X11 keycode - 8); Windows and macOS have their
//! own scancodes, so they use this table. Layout independent: it names the key position, not the letter.
use winit::keyboard::KeyCode;

/// The evdev code of a physical key, if NexDesk knows it.
pub fn evdev(code: KeyCode) -> Option<u16> {
    use KeyCode::*;
    Some(match code {
        Escape => 1,
        Digit1 => 2,
        Digit2 => 3,
        Digit3 => 4,
        Digit4 => 5,
        Digit5 => 6,
        Digit6 => 7,
        Digit7 => 8,
        Digit8 => 9,
        Digit9 => 10,
        Digit0 => 11,
        Minus => 12,
        Equal => 13,
        Backspace => 14,
        Tab => 15,
        KeyQ => 16,
        KeyW => 17,
        KeyE => 18,
        KeyR => 19,
        KeyT => 20,
        KeyY => 21,
        KeyU => 22,
        KeyI => 23,
        KeyO => 24,
        KeyP => 25,
        BracketLeft => 26,
        BracketRight => 27,
        Enter => 28,
        ControlLeft => 29,
        KeyA => 30,
        KeyS => 31,
        KeyD => 32,
        KeyF => 33,
        KeyG => 34,
        KeyH => 35,
        KeyJ => 36,
        KeyK => 37,
        KeyL => 38,
        Semicolon => 39,
        Quote => 40,
        Backquote => 41,
        ShiftLeft => 42,
        Backslash => 43,
        KeyZ => 44,
        KeyX => 45,
        KeyC => 46,
        KeyV => 47,
        KeyB => 48,
        KeyN => 49,
        KeyM => 50,
        Comma => 51,
        Period => 52,
        Slash => 53,
        ShiftRight => 54,
        NumpadMultiply => 55,
        AltLeft => 56,
        Space => 57,
        CapsLock => 58,
        F1 => 59,
        F2 => 60,
        F3 => 61,
        F4 => 62,
        F5 => 63,
        F6 => 64,
        F7 => 65,
        F8 => 66,
        F9 => 67,
        F10 => 68,
        NumLock => 69,
        ScrollLock => 70,
        Numpad7 => 71,
        Numpad8 => 72,
        Numpad9 => 73,
        NumpadSubtract => 74,
        Numpad4 => 75,
        Numpad5 => 76,
        Numpad6 => 77,
        NumpadAdd => 78,
        Numpad1 => 79,
        Numpad2 => 80,
        Numpad3 => 81,
        Numpad0 => 82,
        NumpadDecimal => 83,
        IntlBackslash => 86,
        F11 => 87,
        F12 => 88,
        NumpadEnter => 96,
        ControlRight => 97,
        NumpadDivide => 98,
        PrintScreen => 99,
        AltRight => 100,
        Home => 102,
        ArrowUp => 103,
        PageUp => 104,
        ArrowLeft => 105,
        ArrowRight => 106,
        End => 107,
        ArrowDown => 108,
        PageDown => 109,
        Insert => 110,
        Delete => 111,
        Pause => 119,
        SuperLeft => 125,
        SuperRight => 126,
        ContextMenu => 127,
        _ => return None,
    })
}

/// The code to send for `code`: the windowing system's own number on Linux, the table elsewhere.
pub fn wire_code(code: KeyCode) -> Option<u16> {
    #[cfg(target_os = "linux")]
    {
        use winit::keyboard::PhysicalKey;
        use winit::platform::scancode::PhysicalKeyExtScancode;
        // winit's raw scancode on Linux is the X11 keycode = evdev code + 8
        if let Some(sc) = PhysicalKey::Code(code).to_scancode() {
            if sc >= 8 {
                return Some((sc - 8) as u16);
            }
        }
    }
    evdev(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn well_known_keys_have_their_evdev_numbers() {
        assert_eq!(evdev(KeyCode::KeyA), Some(30));
        assert_eq!(evdev(KeyCode::Enter), Some(28));
        assert_eq!(evdev(KeyCode::ControlLeft), Some(29));
        assert_eq!(evdev(KeyCode::AltLeft), Some(56));
        assert_eq!(evdev(KeyCode::Delete), Some(111));
        assert_eq!(evdev(KeyCode::F12), Some(88));
        assert_eq!(evdev(KeyCode::ArrowLeft), Some(105));
        assert_eq!(evdev(KeyCode::AudioVolumeUp), None);
    }

    #[test]
    fn no_two_keys_share_a_number() {
        let mut seen = std::collections::HashMap::new();
        for c in [
            KeyCode::KeyA, KeyCode::KeyB, KeyCode::Digit1, KeyCode::Space, KeyCode::Numpad0, KeyCode::NumpadEnter,
            KeyCode::Enter, KeyCode::ShiftLeft, KeyCode::ShiftRight, KeyCode::SuperLeft, KeyCode::SuperRight,
            KeyCode::PageUp, KeyCode::PageDown, KeyCode::Home, KeyCode::End, KeyCode::Insert,
        ] {
            let n = evdev(c).unwrap();
            assert!(seen.insert(n, c).is_none(), "{c:?} repeats {n}");
        }
    }
}
