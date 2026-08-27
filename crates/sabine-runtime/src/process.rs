use std::{ffi::OsStr, process::Command};

pub fn background_command(program: impl AsRef<OsStr>) -> Command {
    let mut command = Command::new(program);
    configure_background_command(&mut command);
    command
}

pub fn configure_background_command(command: &mut Command) {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(windows::Win32::System::Threading::CREATE_NO_WINDOW.0);
    }
    #[cfg(not(target_os = "windows"))]
    let _ = command;
}

pub(crate) fn process_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    #[cfg(unix)]
    {
        let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
        result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
    #[cfg(windows)]
    {
        use windows::Win32::{
            Foundation::{CloseHandle, STILL_ACTIVE},
            System::Threading::{
                GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
            },
        };
        let Ok(process) = (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) })
        else {
            return false;
        };
        let mut exit_code = 0;
        let active = unsafe { GetExitCodeProcess(process, &mut exit_code) }.is_ok()
            && exit_code == STILL_ACTIVE.0 as u32;
        let _ = unsafe { CloseHandle(process) };
        active
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
        false
    }
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;

    #[test]
    fn background_children_do_not_inherit_a_console() {
        use windows::Win32::System::Console::GetConsoleWindow;

        if unsafe { GetConsoleWindow() }.is_invalid() {
            return;
        }
        let status = background_command("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Add-Type -Name Native -Namespace Sabine -MemberDefinition '[DllImport(\"kernel32.dll\")] public static extern IntPtr GetConsoleWindow();'; if ([Sabine.Native]::GetConsoleWindow() -eq [IntPtr]::Zero) { exit 0 } else { exit 1 }",
            ])
            .status()
            .expect("PowerShell should launch");
        assert!(status.success());
    }
}
