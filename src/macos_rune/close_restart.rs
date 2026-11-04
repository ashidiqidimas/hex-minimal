//! Restarts Hex in the background once its window closes. Drawing the
//! window leaves memory a closed window can't give back (Metal's
//! command-buffer pools on the process's one GPU device, Rune's last frame,
//! the heap its peak grew), so a fresh process is the only way back to the
//! footprint Hex had before the window opened. The restart is `hex
//! restart`'s own, refused while a meeting runs or a dictation records or
//! transcribes, so it waits and tries again until Hex is idle. The shortcut
//! works again about 0.8 s later.

use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use crate::control::{ControlCall, ControlCommand, ControlReply, ControlRequest, ControlResponse};

/// How long a refused restart waits before asking again.
const RETRY: Duration = Duration::from_secs(2);

#[derive(Default)]
pub(super) struct CloseRestart {
    open: bool,
    /// When to ask for the restart; `None` while nothing is owed.
    due: Option<Instant>,
    /// The answer to the restart already asked for.
    asked: Option<Receiver<ControlResponse>>,
}

impl CloseRestart {
    /// Follows the window each pump tick; returns the restart to send once
    /// it closed and the last one asked was refused or never asked.
    pub(super) fn tick(
        &mut self,
        open: bool,
        setup_done: impl FnOnce() -> bool,
        now: Instant,
    ) -> Option<ControlCall> {
        if open {
            self.open = true;
            self.due = None;
            return None;
        }
        if std::mem::take(&mut self.open) && setup_done() {
            self.due = Some(now);
        }
        if let Some(asked) = &self.asked {
            match asked.try_recv() {
                Ok(response) => {
                    self.asked = None;
                    match response.reply {
                        // Busy or a meeting: ask again once it may be idle.
                        ControlReply::Error { code, .. } if code != "unsupported" => {
                            self.due = Some(now + RETRY);
                        }
                        // Restarting, or a Hex that can't restart itself.
                        _ => self.due = None,
                    }
                }
                Err(mpsc::TryRecvError::Empty) => return None,
                Err(mpsc::TryRecvError::Disconnected) => self.asked = None,
            }
        }
        if self.due.is_none_or(|due| now < due) {
            return None;
        }
        self.due = None;
        let (reply, answer) = mpsc::sync_channel(1);
        self.asked = Some(answer);
        Some(ControlCall {
            request: ControlRequest::new(ControlCommand::Restart { force: false }),
            reply,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(call: ControlCall, reply: ControlReply) {
        call.reply.send(reply.into()).unwrap();
    }

    #[test]
    fn closing_the_window_asks_for_a_restart_once() {
        let mut restart = CloseRestart::default();
        let now = Instant::now();
        assert!(restart.tick(false, || true, now).is_none(), "never opened");
        assert!(restart.tick(true, || true, now).is_none());
        let call = restart.tick(false, || true, now).expect("closed");
        assert_eq!(
            call.request.command,
            ControlCommand::Restart { force: false }
        );
        assert!(restart.tick(false, || true, now).is_none(), "still waiting");
        answer(call, ControlReply::done("Restarting Hex"));
        assert!(restart.tick(false, || true, now + RETRY).is_none());
    }

    #[test]
    fn a_busy_hex_is_asked_again_until_it_restarts() {
        let mut restart = CloseRestart::default();
        let now = Instant::now();
        restart.tick(true, || true, now);
        let call = restart.tick(false, || true, now).unwrap();
        answer(
            call,
            ControlReply::error("busy", "a dictation is recording"),
        );
        assert!(
            restart.tick(false, || true, now).is_none(),
            "waits {RETRY:?}"
        );
        assert!(restart.tick(false, || true, now + RETRY).is_some());
    }

    #[test]
    fn reopening_or_an_unfinished_setup_cancels_it() {
        let mut restart = CloseRestart::default();
        let now = Instant::now();
        restart.tick(true, || true, now);
        let call = restart.tick(false, || true, now).unwrap();
        answer(
            call,
            ControlReply::error("meeting-active", "a meeting is recording"),
        );
        restart.tick(true, || true, now);
        assert!(restart.tick(true, || true, now + RETRY).is_none());

        let mut setup = CloseRestart::default();
        setup.tick(true, || false, now);
        assert!(setup.tick(false, || false, now).is_none());

        let mut unsupported = CloseRestart::default();
        unsupported.tick(true, || true, now);
        let call = unsupported.tick(false, || true, now).unwrap();
        answer(
            call,
            ControlReply::error("unsupported", "not an installed app"),
        );
        assert!(unsupported.tick(false, || true, now + RETRY).is_none());
    }
}
