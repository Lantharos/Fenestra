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
        std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
            .output()
            .ok()
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .and_then(|output| output.lines().next().map(str::to_string))
            .and_then(|line| line.split(',').nth(1).map(str::to_string))
            .is_some_and(|value| value.trim_matches('"') == pid.to_string())
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
        false
    }
}
