use std::{
    collections::VecDeque,
    sync::{Condvar, Mutex},
};

use super::protocol::{OsrFrame, OsrMessage, OsrPaintBatch};

const MAX_QUEUED_MESSAGES: usize = 256;
const MESSAGE_DISPATCH_BUDGET: usize = 32;

pub(super) struct MessageQueue {
    state: Mutex<MessageQueueState>,
    space_available: Condvar,
}

#[derive(Default)]
struct MessageQueueState {
    messages: VecDeque<OsrMessage>,
    wake_queued: bool,
}

impl MessageQueue {
    pub(super) fn new() -> Self {
        Self {
            state: Mutex::new(MessageQueueState::default()),
            space_available: Condvar::new(),
        }
    }

    pub(super) fn push(&self, message: OsrMessage) -> bool {
        let Ok(mut state) = self.state.lock() else {
            return false;
        };
        let message = match message {
            OsrMessage::PaintBatch(incoming) => {
                let matching = state
                    .messages
                    .iter_mut()
                    .rev()
                    .take_while(|message| matches!(message, OsrMessage::PaintBatch(_)))
                    .find_map(|message| match message {
                        OsrMessage::PaintBatch(queued) if queued.surface == incoming.surface => {
                            Some(queued)
                        }
                        _ => None,
                    });
                if let Some(queued) = matching {
                    match merge_paint_batch(queued, incoming) {
                        None => return queue_wake(&mut state),
                        Some(incoming) => OsrMessage::PaintBatch(incoming),
                    }
                } else {
                    OsrMessage::PaintBatch(incoming)
                }
            }
            message => message,
        };
        while state.messages.len() >= MAX_QUEUED_MESSAGES {
            let Ok(next) = self.space_available.wait(state) else {
                return false;
            };
            state = next;
        }
        state.messages.push_back(message);
        queue_wake(&mut state)
    }

    pub(super) fn drain_budgeted(&self) -> (VecDeque<OsrMessage>, bool) {
        let Ok(mut state) = self.state.lock() else {
            return (VecDeque::new(), false);
        };
        let count = state.messages.len().min(MESSAGE_DISPATCH_BUDGET);
        let messages = state.messages.drain(..count).collect();
        let remaining = !state.messages.is_empty();
        state.wake_queued = remaining;
        drop(state);
        self.space_available.notify_all();
        (messages, remaining)
    }

    #[cfg(target_os = "linux")]
    pub(super) fn requeue_front(&self, messages: VecDeque<OsrMessage>) -> bool {
        if messages.is_empty() {
            return false;
        }
        let Ok(mut state) = self.state.lock() else {
            return false;
        };
        for message in messages.into_iter().rev() {
            state.messages.push_front(message);
        }
        queue_wake(&mut state)
    }
}

fn queue_wake(state: &mut MessageQueueState) -> bool {
    if state.wake_queued {
        false
    } else {
        state.wake_queued = true;
        true
    }
}

fn merge_paint_batch(queued: &mut OsrPaintBatch, incoming: OsrPaintBatch) -> Option<OsrPaintBatch> {
    if queued.surface != incoming.surface
        || queued.width != incoming.width
        || queued.height != incoming.height
    {
        return Some(incoming);
    }
    queued.x = incoming.x;
    queued.y = incoming.y;
    for frame in incoming.frames {
        queued
            .frames
            .retain(|queued_frame| !frame_covers(&frame, queued_frame));
        queued.frames.push(frame);
    }
    None
}

fn frame_covers(newer: &OsrFrame, older: &OsrFrame) -> bool {
    let newer_right = i64::from(newer.x) + i64::from(newer.width);
    let newer_bottom = i64::from(newer.y) + i64::from(newer.height);
    let older_right = i64::from(older.x) + i64::from(older.width);
    let older_bottom = i64::from(older.y) + i64::from(older.height);
    newer.x <= older.x
        && newer.y <= older.y
        && newer_right >= older_right
        && newer_bottom >= older_bottom
}
