use crate::{AuthState, Error, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LoginSnapshot {
    pub state: AuthState,
    pub busy: bool,
    pub error: Option<Error>,
    pub generation: u64,
}
impl Default for LoginSnapshot {
    fn default() -> Self {
        Self {
            state: AuthState::Disconnected,
            busy: false,
            error: None,
            generation: 0,
        }
    }
}
#[derive(Default)]
pub struct LoginMachine {
    snapshot: LoginSnapshot,
}
impl LoginMachine {
    pub fn snapshot(&self) -> LoginSnapshot {
        self.snapshot.clone()
    }
    pub fn begin(&mut self) -> u64 {
        self.snapshot.generation = self.snapshot.generation.wrapping_add(1);
        self.snapshot.busy = true;
        self.snapshot.error = None;
        self.snapshot.generation
    }
    pub fn finish(&mut self, generation: u64, result: Result<AuthState>) -> bool {
        if generation != self.snapshot.generation {
            return false;
        }
        self.snapshot.busy = false;
        match result {
            Ok(state) => {
                self.snapshot.state = state;
                self.snapshot.error = None;
            }
            Err(error) => self.snapshot.error = Some(error),
        }
        true
    }
    pub fn observe(&mut self, state: AuthState) {
        self.snapshot.busy = matches!(state, AuthState::Browser { .. });
        self.snapshot.state = state;
        self.snapshot.error = None;
    }
    pub fn cancel(&mut self) {
        self.snapshot.generation = self.snapshot.generation.wrapping_add(1);
        self.snapshot.busy = false;
        self.snapshot.error = None;
    }
    pub fn restore(&mut self, snapshot: LoginSnapshot) {
        self.snapshot = snapshot;
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancelled_and_superseded_results_cannot_complete_a_different_login() {
        let mut machine = LoginMachine::default();
        let old = machine.begin();
        machine.cancel();
        let current = machine.begin();
        assert!(!machine.finish(old, Ok(AuthState::Closed)));
        assert!(machine.snapshot().busy);
        assert!(machine.finish(current, Err(Error::Unauthorized)));
        assert!(!machine.snapshot().busy);
        assert!(machine.snapshot().error.is_some());
        machine.observe(AuthState::Choice {
            step: "method".into(),
            choices: vec![],
        });
        assert!(machine.snapshot().error.is_none());
    }
}
