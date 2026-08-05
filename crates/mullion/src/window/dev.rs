//! Shared Vite/dev-server helpers for builder, manifest, and CLI.

pub fn vite_dev_url(port: u16) -> String {
    format!("http://localhost:{port}")
}

pub fn vite_dev_command(port: u16, package_manager: &str) -> String {
    let pm = if package_manager.trim().is_empty() {
        "bun"
    } else {
        package_manager.trim()
    };
    format!("{pm} run dev -- --port {port} --strictPort")
}

pub fn parse_localhost_port(url: &str) -> Option<u16> {
    let url = url.trim();
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))?;
    let host_port = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    if let Some((host, port)) = host_port.rsplit_once(':') {
        if matches!(host, "localhost" | "127.0.0.1" | "[::1]" | "::1") {
            return port.parse().ok();
        }
    }
    None
}
