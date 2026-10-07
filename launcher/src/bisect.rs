//! "Find the problem mod": halves the list of suspects each test session until
//! one mod is left. Test sessions switch mods off with launch arguments only,
//! so the player's own settings never change.
//!
//! Suspects are kept in load order (dependencies first), and each test keeps
//! the first half on. That half never depends on the half that's off, so no
//! mod is ever tested without what it needs.

use serde::{Deserialize, Serialize};

use crate::doctor::{self, Report};
use crate::game::UserSettings;
use crate::loadorder::{Analysis, LOAD_ORDER_NS};
use crate::mods::{ModInfo, ModKind};

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub enum Target {
    /// a specific error, matched by its message with numbers folded
    Error { message: String, key: String },
    /// the game not closing normally
    Crash,
}

impl Target {
    pub fn describe(&self) -> String {
        match self {
            Target::Error { message, .. } => format!("the error “{}”", message.chars().take(90).collect::<String>()),
            Target::Crash => "the crash".into(),
        }
    }

    /// Did this session show the problem?
    pub fn seen_in(&self, report: &Report) -> bool {
        match self {
            Target::Error { key, .. } => report.errors.iter().any(|e| &doctor::error_key(e) == key),
            Target::Crash => report.found && !report.clean_exit,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Bisect {
    pub target: Target,
    /// namespaces that might be the cause, in load order
    pub suspects: Vec<String>,
    /// the suspects left on in the current test
    pub testing: Vec<String>,
    pub round: u32,
    /// a test session was started and hasn't been judged yet
    pub launched: bool,
    /// what the log of the last test session suggests (the player confirms)
    pub seen: Option<bool>,
    pub found: Option<Vec<String>>,
}

impl Bisect {
    /// Starts with every switched-on mod that isn't part of the base game.
    pub fn start(target: Target, mods: &[ModInfo], analysis: &Analysis) -> Self {
        let suspects: Vec<String> = analysis
            .order
            .iter()
            .map(|&i| &mods[i])
            .filter(|m| m.kind != ModKind::Base && m.namespace != LOAD_ORDER_NS)
            .map(|m| m.namespace.clone())
            .collect();
        let mut b = Bisect { target, suspects, testing: Vec::new(), round: 0, launched: false, seen: None, found: None };
        b.plan();
        b
    }

    fn plan(&mut self) {
        if self.suspects.len() <= 1 {
            self.found = Some(self.suspects.clone());
            self.testing.clear();
            return;
        }
        self.round += 1;
        let half = self.suspects.len().div_ceil(2);
        self.testing = self.suspects[..half].to_vec();
    }

    /// The rounds left, worst case.
    pub fn rounds_left(&self) -> u32 {
        (self.suspects.len().max(1) as f32).log2().ceil() as u32
    }

    pub fn off(&self) -> Vec<&str> {
        self.suspects.iter().filter(|s| !self.testing.contains(s)).map(String::as_str).collect()
    }

    /// Command-line overrides that switch the untested suspects off for one session.
    pub fn launch_args(&self, mods: &[ModInfo], settings: &UserSettings) -> Vec<String> {
        let off = self.off();
        mods.iter()
            .filter(|m| m.is_enabled(settings) && (off.contains(&m.namespace.as_str()) || m.namespace == LOAD_ORDER_NS))
            .map(|m| format!("--{}=false", m.enabled_key()))
            .collect()
    }

    /// The player's verdict on the last test: did the problem show up?
    pub fn answer(&mut self, problem_seen: bool) {
        self.suspects = if problem_seen {
            self.testing.clone()
        } else {
            self.suspects.iter().filter(|s| !self.testing.contains(s)).cloned().collect()
        };
        self.launched = false;
        self.seen = None;
        self.plan();
    }
}

/// Launch arguments for safe mode: every mod that isn't part of the base game off.
pub fn safe_mode_args(mods: &[ModInfo], settings: &UserSettings) -> Vec<String> {
    mods.iter()
        .filter(|m| m.kind != ModKind::Base && m.is_enabled(settings))
        .map(|m| format!("--{}=false", m.enabled_key()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn narrows_to_one_mod() {
        let names = ["ace", "a", "b", "c", "d"];
        let mut b = Bisect {
            target: Target::Crash,
            suspects: names.iter().map(|s| s.to_string()).collect(),
            testing: Vec::new(),
            round: 0,
            launched: false,
            seen: None,
            found: None,
        };
        b.plan();
        // the culprit is "c": a test shows the problem only while "c" is on
        while b.found.is_none() {
            let seen = b.testing.iter().any(|s| s == "c");
            b.answer(seen);
            assert!(b.round <= 4);
        }
        assert_eq!(b.found, Some(vec!["c".to_string()]));
    }

    #[test]
    fn first_half_keeps_dependencies() {
        // load order puts ACE first; every addon depends on it
        let mut b = Bisect {
            target: Target::Crash,
            suspects: vec!["stonehearth_ace".into(), "addon1".into(), "addon2".into()],
            testing: Vec::new(),
            round: 0,
            launched: false,
            seen: None,
            found: None,
        };
        b.plan();
        assert_eq!(b.testing, vec!["stonehearth_ace".to_string(), "addon1".to_string()]);
        assert_eq!(b.off(), vec!["addon2"]);
    }
}
