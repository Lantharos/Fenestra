use std::{
    collections::VecDeque,
    io::{self, Write},
    sync::{Arc, Condvar, Mutex},
    thread,
};

use super::transport::IpcStream;

const MAX_QUEUED_CONTROLS: usize = 256;

pub(super) struct ControlWriter {
    queue: Arc<ControlQueue>,
}

struct ControlQueue {
    state: Mutex<ControlQueueState>,
    ready: Condvar,
}

struct ControlQueueState {
    messages: VecDeque<ControlMessage>,
    closed: bool,
    error: Option<String>,
}

enum ControlMessage {
    Motion(String),
    Ordered {
        line: String,
        coalescing_key: Option<ControlCoalescingKey>,
    },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ControlCoalescingKey {
    Focus,
    Lifecycle,
    Resize,
}

impl ControlWriter {
    pub(super) fn start(mut stream: IpcStream) -> Self {
        let queue = Arc::new(ControlQueue::new());
        let worker_queue = Arc::clone(&queue);
        thread::spawn(move || {
            while let Some(message) = worker_queue.next() {
                if let Err(error) = stream.write_all(message.into_line().as_bytes()) {
                    worker_queue.fail(error);
                    break;
                }
            }
        });
        Self { queue }
    }

    pub(super) fn send(&self, line: String) -> Result<(), String> {
        self.queue.push_ordered(line)
    }

    pub(super) fn send_motion(&self, line: String) -> Result<(), String> {
        self.queue.push_motion(line)
    }
}

impl Drop for ControlWriter {
    fn drop(&mut self) {
        if let Ok(mut state) = self.queue.state.lock() {
            state.closed = true;
        }
        self.queue.ready.notify_one();
    }
}

impl ControlQueue {
    fn new() -> Self {
        Self {
            state: Mutex::new(ControlQueueState {
                messages: VecDeque::new(),
                closed: false,
                error: None,
            }),
            ready: Condvar::new(),
        }
    }

    fn push_ordered(&self, line: String) -> Result<(), String> {
        let coalescing_key = control_coalescing_key(&line);
        let mut state = self.lock_open_state()?;
        if let Some(key) = coalescing_key
            && let Some(index) = state
                .messages
                .iter()
                .enumerate()
                .rev()
                .take_while(|(_, message)| message.is_coalescible_state())
                .find_map(|(index, message)| message.has_coalescing_key(key).then_some(index))
        {
            state.messages.remove(index);
        }
        if state.messages.len() >= MAX_QUEUED_CONTROLS {
            let Some(index) = state.messages.iter().position(ControlMessage::is_motion) else {
                return Err("control queue is full".to_string());
            };
            state.messages.remove(index);
        }
        state.messages.push_back(ControlMessage::Ordered {
            line,
            coalescing_key,
        });
        drop(state);
        self.ready.notify_one();
        Ok(())
    }

    fn push_motion(&self, line: String) -> Result<(), String> {
        let mut state = self.lock_open_state()?;
        if let Some(ControlMessage::Motion(pending)) = state.messages.back_mut() {
            *pending = line;
        } else if state.messages.len() < MAX_QUEUED_CONTROLS {
            state.messages.push_back(ControlMessage::Motion(line));
        } else if let Some(index) = state.messages.iter().position(ControlMessage::is_motion) {
            state.messages.remove(index);
            state.messages.push_back(ControlMessage::Motion(line));
        }
        drop(state);
        self.ready.notify_one();
        Ok(())
    }

    fn lock_open_state(&self) -> Result<std::sync::MutexGuard<'_, ControlQueueState>, String> {
        let state = self
            .state
            .lock()
            .map_err(|_| "control queue lock was poisoned".to_string())?;
        if state.closed {
            return Err(state
                .error
                .clone()
                .unwrap_or_else(|| "control writer is closed".to_string()));
        }
        Ok(state)
    }

    fn next(&self) -> Option<ControlMessage> {
        let mut state = self.state.lock().ok()?;
        loop {
            if let Some(message) = state.messages.pop_front() {
                return Some(message);
            }
            if state.closed {
                return None;
            }
            state = self.ready.wait(state).ok()?;
        }
    }

    fn fail(&self, error: io::Error) {
        if let Ok(mut state) = self.state.lock() {
            state.closed = true;
            state.error = Some(error.to_string());
        }
        self.ready.notify_all();
    }
}

impl ControlMessage {
    fn is_motion(&self) -> bool {
        matches!(self, Self::Motion(_))
    }

    fn is_coalescible_state(&self) -> bool {
        matches!(
            self,
            Self::Ordered {
                coalescing_key: Some(_),
                ..
            }
        )
    }

    fn has_coalescing_key(&self, key: ControlCoalescingKey) -> bool {
        matches!(
            self,
            Self::Ordered {
                coalescing_key: Some(pending),
                ..
            } if *pending == key
        )
    }

    fn into_line(self) -> String {
        match self {
            Self::Motion(line) | Self::Ordered { line, .. } => line,
        }
    }
}

fn control_coalescing_key(line: &str) -> Option<ControlCoalescingKey> {
    match line.split_once('\t').map_or(line, |(command, _)| command) {
        "focus" => Some(ControlCoalescingKey::Focus),
        "lifecycle" => Some(ControlCoalescingKey::Lifecycle),
        "resize" => Some(ControlCoalescingKey::Resize),
        _ => None,
    }
}
