//! Global UI session management and event dispatch.

pub mod app;
pub mod event;
pub mod state;
pub mod theme;
pub mod view;

use std::env;
use std::io::IsTerminal;
use std::sync::mpsc::{self, Sender};
use std::sync::{Mutex, OnceLock};
use std::thread::JoinHandle;

pub use event::UiEvent;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiMode {
    Auto,
    On,
    Off,
}

impl UiMode {
    fn from_env() -> Self {
        match env::var("RALPH_UI") {
            Ok(raw) => match raw.trim().to_ascii_lowercase().as_str() {
                "1" | "true" | "on" => Self::On,
                "0" | "false" | "off" => Self::Off,
                _ => Self::Auto,
            },
            Err(_) => Self::Auto,
        }
    }

    pub fn resolve(no_ui_flag: bool) -> Self {
        if no_ui_flag {
            Self::Off
        } else {
            Self::from_env()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiPromptResult {
    Input(String),
    Exit,
    Interrupted,
}

pub(super) enum UiCommand {
    Event(UiEvent),
    PromptMultiline {
        title: String,
        hint: String,
        choices: Option<Vec<String>>,
        reply: Sender<UiPromptResult>,
    },
    Confirm {
        title: String,
        prompt: String,
        default_yes: bool,
        reply: Sender<bool>,
    },
    ShowExplorer {
        title: String,
        lines: Vec<String>,
        reply: Sender<()>,
    },
    Shutdown,
}

struct UiSession {
    tx: Sender<UiCommand>,
    handle: JoinHandle<()>,
}

fn ui_slot() -> &'static Mutex<Option<UiSession>> {
    static SLOT: OnceLock<Mutex<Option<UiSession>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

fn should_enable(mode: UiMode) -> bool {
    should_enable_for(
        mode,
        std::io::stdout().is_terminal(),
        std::io::stderr().is_terminal(),
    )
}

fn should_enable_for(mode: UiMode, stdout_is_tty: bool, stderr_is_tty: bool) -> bool {
    match mode {
        UiMode::Off => false,
        UiMode::Auto | UiMode::On => stdout_is_tty && stderr_is_tty,
    }
}

fn sender() -> Option<Sender<UiCommand>> {
    let slot = ui_slot();
    let guard = slot.lock().ok()?;
    guard.as_ref().map(|s| s.tx.clone())
}

pub fn emit(event: UiEvent) {
    if let Some(tx) = sender() {
        let _ = tx.send(UiCommand::Event(event));
    }
}

pub fn is_active() -> bool {
    ui_slot()
        .lock()
        .map(|guard| guard.is_some())
        .unwrap_or(false)
}

pub fn stop() {
    let session = {
        let mut guard = match ui_slot().lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        guard.take()
    };

    if let Some(session) = session {
        let _ = session.tx.send(UiCommand::Shutdown);
        let _ = session.handle.join();
    }
}

/// Show a multiline input modal on the active UI.
pub fn prompt_multiline(title: &str, hint: &str) -> Option<UiPromptResult> {
    let tx = sender()?;
    let (reply_tx, reply_rx) = mpsc::channel();
    tx.send(UiCommand::PromptMultiline {
        title: title.to_string(),
        hint: hint.to_string(),
        choices: None,
        reply: reply_tx,
    })
    .ok()?;
    reply_rx.recv().ok()
}

/// Show a yes/no confirmation modal on the active UI.
pub fn prompt_confirm(title: &str, prompt: &str, default_yes: bool) -> Option<bool> {
    let tx = sender()?;
    let (reply_tx, reply_rx) = mpsc::channel();
    tx.send(UiCommand::Confirm {
        title: title.to_string(),
        prompt: prompt.to_string(),
        default_yes,
        reply: reply_tx,
    })
    .ok()?;
    reply_rx.recv().ok()
}

/// Show a full-screen explorer view and wait for user dismissal.
pub fn show_explorer(title: &str, lines: Vec<String>) -> bool {
    let Some(tx) = sender() else {
        return false;
    };
    let (reply_tx, reply_rx) = mpsc::channel();
    if tx
        .send(UiCommand::ShowExplorer {
            title: title.to_string(),
            lines,
            reply: reply_tx,
        })
        .is_err()
    {
        return false;
    }
    reply_rx.recv().is_ok()
}

/// RAII guard for a running UI session.
pub struct UiGuard {
    active: bool,
}

impl UiGuard {
    pub fn is_active(&self) -> bool {
        self.active
    }
}

impl Drop for UiGuard {
    fn drop(&mut self) {
        if self.active {
            stop();
            self.active = false;
        }
    }
}

/// Start the global UI session if mode + terminal conditions allow it.
pub fn start(mode: UiMode) -> UiGuard {
    if !should_enable(mode) {
        return UiGuard { active: false };
    }

    let slot = ui_slot();
    let mut guard = match slot.lock() {
        Ok(g) => g,
        Err(_) => return UiGuard { active: false },
    };

    if guard.is_some() {
        return UiGuard { active: true };
    }

    let (tx, rx) = mpsc::channel::<UiCommand>();
    let handle = std::thread::spawn(move || {
        let _ = app::run(rx);
    });

    *guard = Some(UiSession { tx, handle });
    UiGuard { active: true }
}

#[cfg(test)]
mod tests {
    use super::UiMode;

    #[test]
    fn env_parser_defaults_to_auto_for_unknown() {
        std::env::set_var("RALPH_UI", "something-else");
        assert_eq!(UiMode::resolve(false), UiMode::Auto);
        std::env::remove_var("RALPH_UI");
    }

    #[test]
    fn no_ui_flag_wins() {
        std::env::set_var("RALPH_UI", "1");
        assert_eq!(UiMode::resolve(true), UiMode::Off);
        std::env::remove_var("RALPH_UI");
    }

    #[test]
    fn prompt_calls_fallback_when_ui_not_running() {
        assert!(super::prompt_multiline("T", "H").is_none());
        assert!(super::prompt_confirm("T", "P", true).is_none());
        assert!(!super::show_explorer("X", vec!["a".to_string()]));
    }

    #[test]
    fn non_tty_fallback_matrix() {
        assert!(!super::should_enable_for(UiMode::Off, true, true));
        assert!(!super::should_enable_for(UiMode::On, false, true));
        assert!(!super::should_enable_for(UiMode::On, true, false));
        assert!(!super::should_enable_for(UiMode::Auto, false, false));
        assert!(super::should_enable_for(UiMode::Auto, true, true));
    }
}
