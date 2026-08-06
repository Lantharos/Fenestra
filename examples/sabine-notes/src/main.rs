use std::path::PathBuf;

use sabine::{
    BridgeCommandDescriptor, BridgeResponse, RuntimeConfig, RuntimeMode, SabineLifecyclePolicy,
    SabineWindow, SabineWindowControlAction, WindowRegion, WindowRegionRect,
    run_sabine_host_from_args, user_runtime_path,
};

const APP_TITLEBAR_HEIGHT: i32 = 38;
const SIDEBAR_WIDTH: i32 = 260;

fn main() {
    let args = std::env::args().collect::<Vec<_>>();
    if run_sabine_host_from_args(&args) {
        return;
    }

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let runtime = RuntimeConfig {
        mode: RuntimeMode::SharedPreferred,
        allow_user_install: true,
        bundled_dir: Some(manifest_dir.clone()),
        ..RuntimeConfig::default()
    };
    let mode = ExampleChromeMode::from_args(&args);
    let entry = mode.entry(&manifest_dir);
    let mut window = mode.apply(
        SabineWindow::new()
            .app_id("com.sabine.notes")
            .title("Sabine Notes")
            .size(900, 640)
            .entry(entry)
            .runtime(runtime.clone())
            .lifecycle_policy(SabineLifecyclePolicy::browser_tab())
            .bridge_descriptor_handler(
                BridgeCommandDescriptor::new("notes.create").target("desktop"),
                |command| {
                    let id = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|duration| duration.as_nanos())
                        .unwrap_or_default();
                    Ok(BridgeResponse::json(serde_json::json!({
                        "ok": true,
                        "id": format!("sabine-{id}"),
                        "params": command.params
                    })))
                },
            ),
    );
    if args.iter().any(|arg| arg == "--hidden") {
        window = window.hidden();
    }

    println!("Sabine standalone notes example");
    println!("chrome mode: {}", mode.label());
    println!("user runtime dir: {}", user_runtime_path().display());
    match window.launch() {
        Ok(process) => {
            println!("launched Sabine process {}", process.id());
            let _ = process.wait();
        }
        Err(error) => {
            eprintln!("failed to launch Sabine window: {error}");
            std::process::exit(1);
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum ExampleChromeMode {
    System,
    SabineChrome,
    Frameless,
    Glass,
}

impl ExampleChromeMode {
    fn from_args(args: &[String]) -> Self {
        if args.iter().any(|arg| arg == "--system") {
            Self::System
        } else if args.iter().any(|arg| arg == "--sabine-chrome") {
            Self::SabineChrome
        } else if args.iter().any(|arg| arg == "--frameless") {
            Self::Frameless
        } else if args.iter().any(|arg| arg == "--glass") {
            Self::Glass
        } else {
            Self::Glass
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::System => "system window",
            Self::SabineChrome => "Sabine chrome window",
            Self::Frameless => "app-drawn frameless window",
            Self::Glass => "native chrome glass window",
        }
    }

    fn entry(self, manifest_dir: &std::path::Path) -> String {
        let path = manifest_dir.join("ui/index.html");
        let suffix = if self.uses_app_chrome() {
            "?chrome=app"
        } else {
            ""
        };
        format!("{}{}", path.display(), suffix)
    }

    fn uses_app_chrome(self) -> bool {
        matches!(self, Self::Frameless | Self::SabineChrome)
    }

    fn apply(self, window: SabineWindow) -> SabineWindow {
        match self {
            Self::System => window.system_chrome().opaque(),
            Self::SabineChrome => app_chrome(window.sabine_chrome().opaque()),
            Self::Frameless => app_chrome(
                window
                    .frameless()
                    .glass()
                    .blur_region(WindowRegion::adaptive_titlebar_sidebar(
                        SIDEBAR_WIDTH,
                        APP_TITLEBAR_HEIGHT,
                        14,
                    ))
                    .opaque_region(WindowRegion::adaptive_content_after_sidebar(
                        SIDEBAR_WIDTH,
                        APP_TITLEBAR_HEIGHT,
                    ))
                    .input_region(WindowRegion::adaptive_rounded_rect(14)),
            ),
            // Native Win32 titlebar + DWM Acrylic (default glass). Use
            // `.glass_spec(GlassSpec::new().windows("mica"))` for Mica.
            Self::Glass => window
                .system_chrome()
                .glass()
                .blur_region(WindowRegion::adaptive_titlebar_sidebar(SIDEBAR_WIDTH, 0, 0))
                .opaque_region(WindowRegion::adaptive_content_after_sidebar(
                    SIDEBAR_WIDTH,
                    0,
                )),
        }
    }
}

fn app_chrome(window: SabineWindow) -> SabineWindow {
    window
        .drag_region(WindowRegionRect::new(0, 0, i32::MAX, APP_TITLEBAR_HEIGHT))
        .control_region(
            SabineWindowControlAction::Minimize,
            WindowRegionRect::new(-138, 0, 46, APP_TITLEBAR_HEIGHT),
        )
        .control_region(
            SabineWindowControlAction::Maximize,
            WindowRegionRect::new(-92, 0, 46, APP_TITLEBAR_HEIGHT),
        )
        .control_region(
            SabineWindowControlAction::Close,
            WindowRegionRect::new(-46, 0, 46, APP_TITLEBAR_HEIGHT),
        )
}
