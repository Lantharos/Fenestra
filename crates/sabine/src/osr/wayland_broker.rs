use std::{
    collections::VecDeque,
    env, io,
    io::IoSliceMut,
    mem::MaybeUninit,
    os::{
        fd::{AsRawFd, BorrowedFd, OwnedFd, RawFd},
        unix::process::CommandExt,
    },
    process::Command,
    sync::Mutex,
};

use rustix::net::{RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags, SendFlags, recvmsg, send};

pub(crate) const BROKER_FD_ENV: &str = "SABINE_WAYLAND_BROKER_FD";
pub(crate) const BROKER_KEY_ENV: &str = "SABINE_WAYLAND_BROKER_KEY";

const CHILD_BROKER_FD: RawFd = 197;
const CHILD_WAYLAND_FD: RawFd = 198;
const CHILD_SOURCE_FD_MIN: RawFd = 199;
static BROKER_IO: Mutex<()> = Mutex::new(());

pub(crate) fn adopt() -> io::Result<()> {
    let Some(fd) = broker_fd()? else {
        return Ok(());
    };
    let flags = rustix::io::fcntl_getfd(fd)?;
    rustix::io::fcntl_setfd(fd, flags | rustix::io::FdFlags::CLOEXEC)?;
    Ok(())
}

pub(crate) fn prepare_child(command: &mut Command, forward_broker: bool) -> io::Result<bool> {
    let Some(broker) = broker_fd()? else {
        return Ok(false);
    };
    adopt()?;
    let wayland = request_connection(broker)?;
    let wayland = rustix::io::fcntl_dupfd_cloexec(&wayland, CHILD_SOURCE_FD_MIN)?;
    let forwarded_broker = forward_broker
        .then(|| rustix::io::fcntl_dupfd_cloexec(broker, CHILD_SOURCE_FD_MIN))
        .transpose()?;

    command
        .env_remove("WAYLAND_DISPLAY")
        .env("WAYLAND_SOCKET", CHILD_WAYLAND_FD.to_string());
    if forward_broker {
        command.env(BROKER_FD_ENV, CHILD_BROKER_FD.to_string());
    } else {
        command.env_remove(BROKER_FD_ENV).env_remove(BROKER_KEY_ENV);
    }

    unsafe {
        command.pre_exec(move || {
            duplicate_for_exec(&wayland, CHILD_WAYLAND_FD)?;
            if let Some(broker) = &forwarded_broker {
                duplicate_for_exec(broker, CHILD_BROKER_FD)?;
            }
            Ok(())
        });
    }
    Ok(true)
}

fn request_connection(broker: BorrowedFd<'static>) -> io::Result<OwnedFd> {
    let _guard = BROKER_IO
        .lock()
        .map_err(|_| io::Error::other("Wayland broker lock was poisoned"))?;
    if send(broker, &[1], SendFlags::NOSIGNAL)? != 1 {
        return Err(io::Error::new(
            io::ErrorKind::WriteZero,
            "Wayland broker request was truncated",
        ));
    }

    let mut byte = [0_u8; 1];
    let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1))];
    let mut control = RecvAncillaryBuffer::new(&mut space);
    let response = recvmsg(
        broker,
        &mut [IoSliceMut::new(&mut byte)],
        &mut control,
        RecvFlags::CMSG_CLOEXEC,
    )?;
    if response.bytes != 1 || byte[0] != 1 {
        return Err(io::Error::new(
            io::ErrorKind::ConnectionAborted,
            "Wayland broker rejected the connection request",
        ));
    }
    let mut descriptors = VecDeque::new();
    descriptors.extend(
        control
            .drain()
            .filter_map(|message| match message {
                RecvAncillaryMessage::ScmRights(fds) => Some(fds),
                _ => None,
            })
            .flatten(),
    );
    descriptors.pop_front().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "Wayland broker response did not contain a connection",
        )
    })
}

fn broker_fd() -> io::Result<Option<BorrowedFd<'static>>> {
    let Some(value) = env::var_os(BROKER_FD_ENV) else {
        return Ok(None);
    };
    let value = value.to_string_lossy();
    let raw = value.parse::<RawFd>().map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{BROKER_FD_ENV} is not a file descriptor"),
        )
    })?;
    if raw < 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{BROKER_FD_ENV} is not a valid file descriptor"),
        ));
    }
    Ok(Some(unsafe { BorrowedFd::borrow_raw(raw) }))
}

fn duplicate_for_exec(source: &OwnedFd, target: RawFd) -> io::Result<()> {
    if source.as_raw_fd() == target {
        let flags = rustix::io::fcntl_getfd(source)?;
        rustix::io::fcntl_setfd(source, flags & !rustix::io::FdFlags::CLOEXEC)?;
        return Ok(());
    }
    if unsafe { libc::dup2(source.as_raw_fd(), target) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
