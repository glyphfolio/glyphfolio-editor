//! Opt-in own-process AppKit event probe. This never posts global CGEvents.
//!
//! The generated NSEvents pass through our NSApplication/window and winit's
//! NSTextInputClient path. They are programmatic native events, not physical
//! keyboard input. Japanese proof additionally requires the actual Japanese
//! input method to be selected by the external, restoring TIS driver.

use std::{
    fmt,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProbeMode {
    Latin,
    JapaneseRomaji,
    JapaneseCancel,
    ChinesePinyin,
    JapaneseFocusLoss,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProbeProgress {
    WaitingForWindow,
    WaitingForNextKey,
    Posted {
        index: usize,
        total: usize,
        key: &'static str,
    },
    Settling,
    Complete,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProbeError {
    #[cfg(not(target_os = "macos"))]
    UnsupportedPlatform,
    #[cfg(target_os = "macos")]
    NotMainThread,
    NoOwnedKeyWindow,
    #[cfg(target_os = "macos")]
    WrongWindowTitle(String),
    #[cfg(target_os = "macos")]
    EventCreationFailed,
}

impl fmt::Display for ProbeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            #[cfg(not(target_os = "macos"))]
            Self::UnsupportedPlatform => formatter.write_str("AppKit input probe requires macOS"),
            #[cfg(target_os = "macos")]
            Self::NotMainThread => {
                formatter.write_str("AppKit input probe must run on the main thread")
            }
            Self::NoOwnedKeyWindow => {
                formatter.write_str("Our native editor did not acquire a key window")
            }
            #[cfg(target_os = "macos")]
            Self::WrongWindowTitle(title) => write!(
                formatter,
                "Refusing key events for unexpected own-window title: {title}"
            ),
            #[cfg(target_os = "macos")]
            Self::EventCreationFailed => {
                formatter.write_str("AppKit declined to construct the own-window key event")
            }
        }
    }
}

impl std::error::Error for ProbeError {}

#[derive(Clone, Copy)]
struct KeyStroke {
    text: &'static str,
    code: u16,
    label: &'static str,
}

const LATIN: &[KeyStroke] = &[
    KeyStroke {
        text: "p",
        code: 35,
        label: "p",
    },
    KeyStroke {
        text: "r",
        code: 15,
        label: "r",
    },
    KeyStroke {
        text: "o",
        code: 31,
        label: "o",
    },
    KeyStroke {
        text: "b",
        code: 11,
        label: "b",
    },
    KeyStroke {
        text: "e",
        code: 14,
        label: "e",
    },
];

const JAPANESE: &[KeyStroke] = &[
    KeyStroke {
        text: "n",
        code: 45,
        label: "n",
    },
    KeyStroke {
        text: "i",
        code: 34,
        label: "i",
    },
    KeyStroke {
        text: "h",
        code: 4,
        label: "h",
    },
    KeyStroke {
        text: "o",
        code: 31,
        label: "o",
    },
    KeyStroke {
        text: "n",
        code: 45,
        label: "n",
    },
    KeyStroke {
        text: " ",
        code: 49,
        label: "Space",
    },
    KeyStroke {
        text: "\r",
        code: 36,
        label: "Return",
    },
];

const JAPANESE_CANCEL: &[KeyStroke] = &[
    KeyStroke {
        text: "n",
        code: 45,
        label: "n",
    },
    KeyStroke {
        text: "i",
        code: 34,
        label: "i",
    },
    KeyStroke {
        text: "h",
        code: 4,
        label: "h",
    },
    KeyStroke {
        text: "o",
        code: 31,
        label: "o",
    },
    KeyStroke {
        text: "n",
        code: 45,
        label: "n",
    },
    KeyStroke {
        text: "\u{1b}",
        code: 53,
        label: "Escape",
    },
];

/// Drive from the native demo's UI frames and request periodic repaint.
/// Wait for [`ProbeProgress::Complete`] before examining editor/logged outcomes.
pub struct NativeKeyProbe {
    keys: &'static [KeyStroke],
    created: Instant,
    next_action: Instant,
    index: usize,
    settle_until: Option<Instant>,
}

const CHINESE: &[KeyStroke] = &[
    KeyStroke {
        text: "z",
        code: 6,
        label: "z",
    },
    KeyStroke {
        text: "h",
        code: 4,
        label: "h",
    },
    KeyStroke {
        text: "o",
        code: 31,
        label: "o",
    },
    KeyStroke {
        text: "n",
        code: 45,
        label: "n",
    },
    KeyStroke {
        text: "g",
        code: 5,
        label: "g",
    },
    KeyStroke {
        text: "w",
        code: 13,
        label: "w",
    },
    KeyStroke {
        text: "e",
        code: 14,
        label: "e",
    },
    KeyStroke {
        text: "n",
        code: 45,
        label: "n",
    },
    KeyStroke {
        text: " ",
        code: 49,
        label: "Space",
    },
    KeyStroke {
        text: "\r",
        code: 36,
        label: "Return",
    },
];

impl NativeKeyProbe {
    pub fn new(mode: ProbeMode) -> Self {
        let created = Instant::now();
        Self {
            keys: match mode {
                ProbeMode::Latin => LATIN,
                ProbeMode::JapaneseRomaji => JAPANESE,
                ProbeMode::JapaneseCancel => JAPANESE_CANCEL,
                ProbeMode::ChinesePinyin => CHINESE,
                ProbeMode::JapaneseFocusLoss => &[], // External observer mode only.
            },
            created,
            // Let eframe create/focus the view and publish its IME rectangle.
            next_action: created + Duration::from_secs(2),
            index: 0,
            settle_until: None,
        }
    }

    pub fn tick(&mut self) -> Result<ProbeProgress, ProbeError> {
        if let Some(until) = self.settle_until {
            return Ok(if Instant::now() >= until {
                ProbeProgress::Complete
            } else {
                ProbeProgress::Settling
            });
        }
        if Instant::now() < self.next_action {
            return Ok(ProbeProgress::WaitingForNextKey);
        }
        let Some(key) = self.keys.get(self.index) else {
            self.settle_until = Some(Instant::now() + Duration::from_millis(900));
            return Ok(ProbeProgress::Settling);
        };
        if !post_own_key(*key)? {
            return if self.created.elapsed() > Duration::from_secs(15) {
                Err(ProbeError::NoOwnedKeyWindow)
            } else {
                Ok(ProbeProgress::WaitingForWindow)
            };
        }
        self.index += 1;
        self.next_action = Instant::now() + Duration::from_millis(220);
        Ok(ProbeProgress::Posted {
            index: self.index,
            total: self.keys.len(),
            key: key.label,
        })
    }
}

#[cfg(target_os = "macos")]
fn post_own_key(key: KeyStroke) -> Result<bool, ProbeError> {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSEvent, NSEventModifierFlags, NSEventType};
    use objc2_foundation::{NSPoint, NSProcessInfo, NSString};

    let marker = MainThreadMarker::new().ok_or(ProbeError::NotMainThread)?;
    let application = NSApplication::sharedApplication(marker);
    let Some(window) = application.keyWindow() else {
        return Ok(false);
    };
    let title = window.title().to_string();
    if !title.contains("glyphfolio-editor") {
        return Err(ProbeError::WrongWindowTitle(title));
    }
    let characters = NSString::from_str(key.text);
    for kind in [NSEventType::KeyDown, NSEventType::KeyUp] {
        let event = NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
            kind,
            NSPoint::new(0.0, 0.0),
            NSEventModifierFlags::empty(),
            NSProcessInfo::processInfo().systemUptime(),
            window.windowNumber(),
            None,
            &characters,
            &characters,
            false,
            key.code,
        ).ok_or(ProbeError::EventCreationFailed)?;
        // Enqueue instead of synchronous sendEvent: avoid entering winit's
        // responder while egui's UI callback still owns the editor/context.
        application.postEvent_atStart(&event, false);
    }
    Ok(true)
}

#[cfg(not(target_os = "macos"))]
fn post_own_key(key: KeyStroke) -> Result<bool, ProbeError> {
    let _ = (key.text, key.code);
    Err(ProbeError::UnsupportedPlatform)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn japanese_probe_contains_romaji_and_conversion_keys_not_injected_japanese_text() {
        assert_eq!(
            JAPANESE[..5].iter().map(|key| key.text).collect::<String>(),
            "nihon"
        );
        assert!(JAPANESE.iter().all(|key| key.text.is_ascii()));
        assert_eq!(JAPANESE[5].code, 49);
        assert_eq!(JAPANESE[6].code, 36);
        assert_eq!(JAPANESE_CANCEL.last().unwrap().code, 53);
    }

    #[test]
    fn probe_has_an_initial_window_input_settle_period() {
        for mode in [
            ProbeMode::Latin,
            ProbeMode::JapaneseRomaji,
            ProbeMode::JapaneseCancel,
        ] {
            let mut probe = NativeKeyProbe::new(mode);
            assert_eq!(probe.tick().unwrap(), ProbeProgress::WaitingForNextKey);
        }
    }
}
