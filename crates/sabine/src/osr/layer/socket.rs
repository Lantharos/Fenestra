use std::{
    io::{self, BufRead},
    net::Shutdown,
    os::fd::AsRawFd,
    os::unix::net::{UnixListener, UnixStream},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use layershellev::calloop::channel::Sender;

use crate::ShellSurfaceMargin;
use crate::osr::message_queue::MessageQueue;
use crate::osr::protocol::read_message;

pub(super) enum LayerHostEvent {
    Connected(UnixStream),
    MessagesReady(Arc<MessageQueue>),
    Visible {
        visible: bool,
        request_id: Option<u64>,
    },
    Presentation {
        visible: bool,
        request_id: u64,
        alpha: f32,
        margin: ShellSurfaceMargin,
    },
    Alpha(f32),
    Margin(ShellSurfaceMargin),
    Size(u32, u32),
    FrameRate(crate::ShellSurfaceFrameRate),
    Quit,
    ControlLine(String),
    Disconnected,
}

pub(super) fn start_layer_parent_bridge_reader(sender: Sender<LayerHostEvent>) {
    thread::spawn(move || {
        let input = io::stdin();
        for line in input.lock().lines().map_while(std::result::Result::ok) {
            if let Some((visible, request_id, alpha, margin)) = parse_presentation_control(&line) {
                if sender
                    .send(LayerHostEvent::Presentation {
                        visible,
                        request_id,
                        alpha,
                        margin,
                    })
                    .is_err()
                {
                    break;
                }
                continue;
            }
            if let Some((visible, request_id)) = parse_visibility_control(&line) {
                if sender
                    .send(LayerHostEvent::Visible {
                        visible,
                        request_id,
                    })
                    .is_err()
                {
                    break;
                }
                continue;
            }
            if let Some(alpha) = parse_alpha_control(&line) {
                if sender.send(LayerHostEvent::Alpha(alpha)).is_err() {
                    break;
                }
                continue;
            }
            if let Some(margin) = parse_margin_control(&line) {
                if sender.send(LayerHostEvent::Margin(margin)).is_err() {
                    break;
                }
                continue;
            }
            if let Some((width, height)) = parse_size_control(&line) {
                if sender.send(LayerHostEvent::Size(width, height)).is_err() {
                    break;
                }
                continue;
            }
            if let Some(frame_rate) = parse_frame_rate_control(&line) {
                if sender.send(LayerHostEvent::FrameRate(frame_rate)).is_err() {
                    break;
                }
                continue;
            }
            if parse_quit_control(&line) {
                let _ = sender.send(LayerHostEvent::Quit);
                break;
            }
            if sender.send(LayerHostEvent::ControlLine(line)).is_err() {
                break;
            }
        }
    });
}

fn parse_presentation_control(line: &str) -> Option<(bool, u64, f32, ShellSurfaceMargin)> {
    let (command, value) = crate::parse_host_control(line)?;
    if command != "presentation" {
        return None;
    }
    let value = serde_json::from_str::<serde_json::Value>(value).ok()?;
    let visible = value.get("visible")?.as_bool()?;
    let request_id = value.get("requestId")?.as_u64()?;
    let alpha = value.get("alpha")?.as_f64()? as f32;
    let margin = value.get("margin")?.as_array()?;
    let [top, right, bottom, left] = margin.as_slice() else {
        return None;
    };
    Some((
        visible,
        request_id,
        alpha.clamp(0.0, 1.0),
        ShellSurfaceMargin {
            top: i32::try_from(top.as_i64()?).ok()?,
            right: i32::try_from(right.as_i64()?).ok()?,
            bottom: i32::try_from(bottom.as_i64()?).ok()?,
            left: i32::try_from(left.as_i64()?).ok()?,
        },
    ))
}

pub(super) struct PendingLayerSocket {
    endpoint: Option<crate::osr::transport::IpcEndpoint>,
    listener: Option<UnixListener>,
}

pub(super) struct LayerSocketHandle {
    endpoint: crate::osr::transport::IpcEndpoint,
    cancelled: Arc<AtomicBool>,
    cancellation: UnixStream,
    armed: bool,
}

impl PendingLayerSocket {
    pub(super) fn bind(app_id: &str) -> Option<Self> {
        let (endpoint, listener) = match crate::osr::transport::IpcEndpoint::bind(app_id) {
            Ok(connection) => connection,
            Err(error) => {
                eprintln!("failed to bind Sabine layer OSR socket: {error}");
                return None;
            }
        };
        Some(Self {
            endpoint: Some(endpoint),
            listener: Some(listener),
        })
    }

    pub(super) fn endpoint(&self) -> Option<&crate::osr::transport::IpcEndpoint> {
        self.endpoint.as_ref()
    }

    pub(super) fn start(
        mut self,
        sender: Sender<LayerHostEvent>,
        authentication_token: String,
    ) -> Option<LayerSocketHandle> {
        let listener = self.listener.take()?;
        let endpoint = self.endpoint.take()?;
        let (cancellation, cancellation_reader) = UnixStream::pair().ok()?;
        let cancelled = Arc::new(AtomicBool::new(false));
        start_socket_reader(
            listener,
            endpoint.clone(),
            authentication_token,
            sender,
            Arc::clone(&cancelled),
            cancellation_reader,
        );
        Some(LayerSocketHandle {
            endpoint,
            cancelled,
            cancellation,
            armed: true,
        })
    }
}

impl Drop for PendingLayerSocket {
    fn drop(&mut self) {
        if let Some(endpoint) = &self.endpoint {
            endpoint.unlink();
        }
    }
}

impl LayerSocketHandle {
    pub(super) fn disarm(mut self) {
        self.armed = false;
    }
}

impl Drop for LayerSocketHandle {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        self.cancelled.store(true, Ordering::Release);
        let _ = self.cancellation.shutdown(Shutdown::Both);
        self.endpoint.unlink();
    }
}

fn start_socket_reader(
    listener: UnixListener,
    endpoint: crate::osr::transport::IpcEndpoint,
    authentication_token: String,
    sender: Sender<LayerHostEvent>,
    cancelled: Arc<AtomicBool>,
    cancellation: UnixStream,
) {
    thread::spawn(move || {
        let messages = Arc::new(MessageQueue::new());
        let mut stream = loop {
            if cancelled.load(Ordering::Acquire) {
                endpoint.unlink();
                return;
            }
            if !wait_for_socket_connection(&listener, &cancellation) {
                endpoint.unlink();
                return;
            }
            let Ok((mut candidate, _)) = listener.accept() else {
                endpoint.unlink();
                return;
            };
            if cancelled.load(Ordering::Acquire) {
                endpoint.unlink();
                return;
            }
            if let Err(error) = candidate.set_read_timeout(Some(Duration::from_millis(750))) {
                eprintln!("Sabine layer OSR could not set authentication deadline: {error}");
                continue;
            }
            match crate::osr::transport::authenticate(&mut candidate, &authentication_token) {
                Ok(crate::osr::transport::Authentication::Accepted) => {
                    if let Err(error) = candidate.set_read_timeout(None) {
                        eprintln!(
                            "Sabine layer OSR could not clear authentication deadline: {error}"
                        );
                        continue;
                    }
                    break candidate;
                }
                Ok(crate::osr::transport::Authentication::Probe) => continue,
                Err(error) => {
                    eprintln!("Sabine layer OSR reject connect: {error}");
                }
            }
        };
        if cancelled.load(Ordering::Acquire) {
            endpoint.unlink();
            let _ = stream.shutdown(Shutdown::Both);
            return;
        }
        if let Ok(writer) = stream.try_clone() {
            let _ = sender.send(LayerHostEvent::Connected(writer));
        }
        loop {
            match read_message(&mut stream) {
                Ok(Some(message)) => {
                    if messages.push(message)
                        && sender
                            .send(LayerHostEvent::MessagesReady(Arc::clone(&messages)))
                            .is_err()
                    {
                        break;
                    }
                }
                Ok(None) => break,
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::BrokenPipe
                    ) =>
                {
                    break;
                }
                Err(error) => {
                    eprintln!("Sabine layer OSR socket read failed: {error}");
                    break;
                }
            }
        }
        endpoint.unlink();
        let _ = stream.shutdown(Shutdown::Both);
        let _ = sender.send(LayerHostEvent::Disconnected);
    });
}

fn wait_for_socket_connection(listener: &UnixListener, cancellation: &UnixStream) -> bool {
    let mut descriptors = [
        libc::pollfd {
            fd: listener.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        },
        libc::pollfd {
            fd: cancellation.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        },
    ];
    loop {
        let result = unsafe {
            libc::poll(
                descriptors.as_mut_ptr(),
                descriptors.len() as libc::nfds_t,
                -1,
            )
        };
        if result > 0 {
            return descriptors[1].revents == 0 && descriptors[0].revents != 0;
        }
        if result == 0 {
            continue;
        }
        if io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
            return false;
        }
    }
}

fn parse_visibility_control(line: &str) -> Option<(bool, Option<u64>)> {
    let (command, value) = crate::parse_host_control(line)?;
    match command {
        "visible" => {
            let (value, request_id) = value
                .split_once(':')
                .map(|(value, request_id)| (value, request_id.parse::<u64>().ok()))
                .unwrap_or((value, None));
            match value {
                "1" | "true" | "yes" | "show" | "visible" => Some((true, request_id)),
                "0" | "false" | "no" | "hide" | "hidden" => Some((false, request_id)),
                _ => None,
            }
        }
        "show" | "focus" => Some((true, None)),
        "hide" => Some((false, None)),
        _ => None,
    }
}

fn parse_quit_control(line: &str) -> bool {
    crate::parse_host_control(line).is_some_and(|(command, _)| command == "quit")
}

fn parse_alpha_control(line: &str) -> Option<f32> {
    let (command, value) = crate::parse_host_control(line)?;
    if command != "alpha" {
        return None;
    }
    value.parse::<f32>().ok().map(|alpha| alpha.clamp(0.0, 1.0))
}

fn parse_margin_control(line: &str) -> Option<ShellSurfaceMargin> {
    let (command, value) = crate::parse_host_control(line)?;
    if command != "margin" {
        return None;
    }
    let mut parts = value.split(',').map(str::trim);
    let top = parts.next()?.parse::<i32>().ok()?;
    let right = parts.next()?.parse::<i32>().ok()?;
    let bottom = parts.next()?.parse::<i32>().ok()?;
    let left = parts.next()?.parse::<i32>().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some(ShellSurfaceMargin {
        top,
        right,
        bottom,
        left,
    })
}

fn parse_size_control(line: &str) -> Option<(u32, u32)> {
    let (command, value) = crate::parse_host_control(line)?;
    if command != "size" {
        return None;
    }
    let (width, height) = value.split_once(',')?;
    Some((
        width.trim().parse::<u32>().ok()?.max(1),
        height.trim().parse::<u32>().ok()?.max(1),
    ))
}

fn parse_frame_rate_control(line: &str) -> Option<crate::ShellSurfaceFrameRate> {
    let (command, value) = crate::parse_host_control(line)?;
    if command != "shell-frame-rate" {
        return None;
    }
    crate::ShellSurfaceFrameRate::new(value.trim().parse().ok()?)
}

#[cfg(test)]
mod tests {
    use super::{parse_frame_rate_control, parse_size_control, parse_visibility_control};

    #[test]
    fn visibility_control_preserves_completion_request_id() {
        assert_eq!(
            parse_visibility_control("SABINE_HOST_CONTROL\tvisible\t1:42"),
            Some((true, Some(42)))
        );
        assert_eq!(
            parse_visibility_control("SABINE_HOST_CONTROL\tvisible\t0"),
            Some((false, None))
        );
    }

    #[test]
    fn size_control_clamps_empty_dimensions() {
        assert_eq!(
            parse_size_control("SABINE_HOST_CONTROL\tsize\t228,363"),
            Some((228, 363))
        );
        assert_eq!(
            parse_size_control("SABINE_HOST_CONTROL\tsize\t0,0"),
            Some((1, 1))
        );
    }

    #[test]
    fn frame_rate_control_validates_bounds() {
        assert_eq!(
            parse_frame_rate_control("SABINE_HOST_CONTROL\tshell-frame-rate\t144")
                .map(|rate| rate.get()),
            Some(144)
        );
        assert!(parse_frame_rate_control("SABINE_HOST_CONTROL\tshell-frame-rate\t0").is_none());
    }
}
