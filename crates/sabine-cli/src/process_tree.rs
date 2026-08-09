use std::{
    io,
    process::{Child, Command, ExitStatus},
    thread,
    time::{Duration, Instant},
};

const TERMINATE_GRACE: Duration = Duration::from_millis(500);

pub struct ProcessTree {
    child: Option<Child>,
    platform: platform::ProcessTree,
}

impl ProcessTree {
    pub fn spawn(command: &mut Command) -> io::Result<Self> {
        platform::prepare(command);
        let mut child = command.spawn()?;
        let platform = match platform::ProcessTree::attach(&child) {
            Ok(platform) => platform,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        };
        Ok(Self {
            child: Some(child),
            platform,
        })
    }

    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        let Some(child) = self.child.as_mut() else {
            return Ok(None);
        };
        let status = child.try_wait()?;
        if status.is_some() {
            self.child = None;
        }
        Ok(status)
    }

    pub fn terminate(&mut self) -> io::Result<Option<ExitStatus>> {
        let Some(child) = self.child.as_mut() else {
            self.platform.terminate();
            self.platform.kill();
            return Ok(None);
        };

        self.platform.terminate();
        let deadline = Instant::now() + TERMINATE_GRACE;
        while Instant::now() < deadline {
            if let Some(status) = child.try_wait()? {
                self.platform.kill();
                self.child = None;
                return Ok(Some(status));
            }
            thread::sleep(Duration::from_millis(10));
        }

        self.platform.kill();
        let status = child.wait()?;
        self.child = None;
        Ok(Some(status))
    }
}

impl Drop for ProcessTree {
    fn drop(&mut self) {
        let _ = self.terminate();
    }
}

#[cfg(unix)]
mod platform {
    use std::{io, os::unix::process::CommandExt, process::Child, process::Command};

    pub struct ProcessTree {
        group: i32,
    }

    pub fn prepare(command: &mut Command) {
        command.process_group(0);
    }

    impl ProcessTree {
        pub fn attach(child: &Child) -> io::Result<Self> {
            let group = i32::try_from(child.id())
                .map_err(|_| io::Error::other("child process id exceeds i32"))?;
            Ok(Self { group })
        }

        pub fn terminate(&self) {
            self.signal(libc::SIGTERM);
        }

        pub fn kill(&self) {
            self.signal(libc::SIGKILL);
        }

        fn signal(&self, signal: i32) {
            unsafe {
                libc::kill(-self.group, signal);
            }
        }
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use std::{
        ffi::c_void,
        io,
        mem::size_of,
        os::windows::{io::AsRawHandle, process::CommandExt},
        process::{Child, Command},
    };

    use windows::{
        Win32::{
            Foundation::{CloseHandle, HANDLE},
            System::{
                JobObjects::{
                    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
                    SetInformationJobObject, TerminateJobObject,
                },
                Threading::CREATE_NEW_PROCESS_GROUP,
            },
        },
        core::PCWSTR,
    };

    pub struct ProcessTree {
        job: HANDLE,
    }

    pub fn prepare(command: &mut Command) {
        command.creation_flags(CREATE_NEW_PROCESS_GROUP.0);
    }

    impl ProcessTree {
        pub fn attach(child: &Child) -> io::Result<Self> {
            let job =
                unsafe { CreateJobObjectW(None, PCWSTR::null()) }.map_err(io::Error::other)?;
            let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let configure = unsafe {
                SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    (&raw const limits).cast::<c_void>(),
                    size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                )
            };
            if let Err(error) = configure {
                unsafe {
                    let _ = CloseHandle(job);
                }
                return Err(io::Error::other(error));
            }

            let process = HANDLE(child.as_raw_handle());
            if let Err(error) = unsafe { AssignProcessToJobObject(job, process) } {
                unsafe {
                    let _ = CloseHandle(job);
                }
                return Err(io::Error::other(error));
            }
            Ok(Self { job })
        }

        pub fn terminate(&self) {
            unsafe {
                let _ = TerminateJobObject(self.job, 1);
            }
        }

        pub fn kill(&self) {
            self.terminate();
        }
    }

    impl Drop for ProcessTree {
        fn drop(&mut self) {
            unsafe {
                let _ = CloseHandle(self.job);
            }
        }
    }
}
