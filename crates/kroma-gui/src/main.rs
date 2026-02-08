//! Kroma GUI — configuration, shader management, and Shadertoy import tool.
//!
//! Runs as an `iced` desktop application that communicates with
//! `kroma-daemon` over Unix IPC.
//!
//! Also supports a CLI fallback mode when `--cli` is passed.

mod importer;
mod ipc_client;

use std::path::PathBuf;

use anyhow::Result;
use iced::widget::{button, column, container, horizontal_rule, row, scrollable, text, text_input};
use iced::{Element, Length, Subscription, Task, Theme};
use log::info;

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let args: Vec<String> = std::env::args().collect();

    // CLI fallback mode: any argument except --gui triggers CLI
    if args.len() > 1 && args[1] != "--gui" {
        return cli_main(&args);
    }

    // Launch the iced GUI
    info!("Kroma GUI v{}", env!("CARGO_PKG_VERSION"));
    iced::application("Kroma", KromaApp::update, KromaApp::view)
        .theme(KromaApp::theme)
        .subscription(KromaApp::subscription)
        .run_with(KromaApp::new)
        .map_err(|e| anyhow::anyhow!("GUI error: {}", e))?;

    Ok(())
}

// ---------------------------------------------------------------------------
// CLI mode (backwards-compatible)
// ---------------------------------------------------------------------------
fn cli_main(args: &[String]) -> Result<()> {
    info!("Kroma GUI v{} (CLI mode)", env!("CARGO_PKG_VERSION"));

    match args.get(1).map(|s| s.as_str()) {
        Some("import") => {
            let source_path = args.get(2).map(PathBuf::from);
            let name = args.get(3).map(|s| s.as_str()).unwrap_or("Imported Shader");
            let author = args.get(4).map(|s| s.as_str()).unwrap_or("Unknown");
            if let Some(path) = source_path {
                info!("Importing shader from: {}", path.display());
                let output = importer::import_shadertoy_file(&path, name, author)?;
                info!("Created shade package: {}", output.display());
            } else {
                eprintln!("Usage: kroma-gui import <shader.glsl> [name] [author]");
            }
        }
        Some("load") => {
            let shade_path = args.get(2).map(|s| s.as_str()).unwrap_or("");
            if shade_path.is_empty() {
                eprintln!("Usage: kroma-gui load <path.shade>");
            } else {
                ipc_client::send_load(shade_path)?;
                info!("Sent load command to daemon: {}", shade_path);
            }
        }
        Some("pause") => {
            ipc_client::send_pause()?;
            info!("Sent pause command");
        }
        Some("resume") => {
            ipc_client::send_resume()?;
            info!("Sent resume command");
        }
        Some("shutdown") => {
            ipc_client::send_shutdown()?;
            info!("Sent shutdown command");
        }
        Some("status") => {
            match ipc_client::query_status() {
                Ok(response) => println!("{}", response),
                Err(e) => eprintln!("Failed: {}", e),
            }
        }
        Some("download") => {
            let url_or_id = args.get(2).map(|s| s.as_str()).unwrap_or("");
            if url_or_id.is_empty() {
                eprintln!("Usage: kroma-gui download <shadertoy-url-or-id> [output-dir]");
            } else {
                let output_dir = args
                    .get(3)
                    .map(PathBuf::from)
                    .unwrap_or_else(dirs_output_dir);
                let rt = tokio::runtime::Runtime::new()?;
                match rt.block_on(importer::download_shadertoy(url_or_id, &output_dir, None)) {
                    Ok((shade_path, name)) => {
                        info!("Downloaded '{}': {}", name, shade_path.display());
                    }
                    Err(e) => eprintln!("Download failed: {}", e),
                }
            }
        }
        _ => {
            println!("Kroma GUI v{}", env!("CARGO_PKG_VERSION"));
            println!();
            println!("Usage:");
            println!("  kroma-gui                                           Launch graphical interface");
            println!("  kroma-gui import <shader.glsl> [name] [author]      Import Shadertoy shader");
            println!("  kroma-gui download <url-or-id> [output-dir]         Download from Shadertoy");
            println!("  kroma-gui load <path.shade>                         Load shade package");
            println!("  kroma-gui pause / resume / shutdown / status        Control daemon");
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Iced application
// ---------------------------------------------------------------------------

struct KromaApp {
    /// Currently loaded shade path
    loaded_shade: Option<String>,
    /// Status text from daemon
    status_text: String,
    /// Whether daemon is connected
    daemon_connected: bool,
    /// Whether rendering is paused
    paused: bool,
    /// FPS reported by daemon
    fps: f32,
    /// Import shader path input
    import_path: String,
    /// Import shader name input
    import_name: String,
    /// Import shader author input
    import_author: String,
    /// Import result message
    import_result: String,
    /// Shadertoy URL/ID input for download
    shadertoy_url: String,
    /// Shadertoy download result message
    download_result: String,
    /// Whether a download is in progress
    downloading: bool,
    /// Log / event messages
    log_messages: Vec<String>,
}

#[derive(Debug, Clone)]
enum Message {
    // Shade management
    LoadShadeClicked,
    ShadeFileSelected(Option<PathBuf>),
    UnloadShadeClicked,

    // Daemon control
    PauseClicked,
    ResumeClicked,
    ShutdownClicked,
    RefreshStatus,
    StatusReceived(String),

    // Import
    ImportPathChanged(String),
    ImportNameChanged(String),
    ImportAuthorChanged(String),
    ImportClicked,
    BrowseImportClicked,
    ImportFileSelected(Option<PathBuf>),
    ImportResult(String),

    // Shadertoy download
    ShadertoyUrlChanged(String),
    DownloadShadertoyClicked,
    DownloadResult(String),

    // Misc
    Tick,
}

impl KromaApp {
    fn new() -> (Self, Task<Message>) {
        let app = Self {
            loaded_shade: None,
            status_text: "Connecting to daemon...".into(),
            daemon_connected: false,
            paused: false,
            fps: 0.0,
            import_path: String::new(),
            import_name: "Imported Shader".into(),
            import_author: "Unknown".into(),
            import_result: String::new(),
            shadertoy_url: String::new(),
            download_result: String::new(),
            downloading: false,
            log_messages: Vec::new(),
        };

        (app, Task::perform(async { query_daemon_status() }, Message::StatusReceived))
    }

    fn theme(&self) -> Theme {
        Theme::Dark
    }

    fn subscription(&self) -> Subscription<Message> {
        iced::time::every(std::time::Duration::from_secs(3)).map(|_| Message::Tick)
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::LoadShadeClicked => {
                return Task::perform(
                    async {
                        let handle = rfd::AsyncFileDialog::new()
                            .set_title("Select Shade Package")
                            .add_filter("Shade Package", &["shade"])
                            .pick_file()
                            .await;
                        handle.map(|f| f.path().to_path_buf())
                    },
                    Message::ShadeFileSelected,
                );
            }

            Message::ShadeFileSelected(Some(path)) => {
                let path_str = path.to_string_lossy().to_string();
                match ipc_client::send_load(&path_str) {
                    Ok(()) => {
                        self.loaded_shade = Some(path_str.clone());
                        self.log_msg(format!("Loaded: {}", path_str));
                    }
                    Err(e) => self.log_msg(format!("Load failed: {}", e)),
                }
            }
            Message::ShadeFileSelected(None) => {}

            Message::UnloadShadeClicked => {
                self.loaded_shade = None;
                self.log_msg("Shade unloaded".into());
            }

            Message::PauseClicked => {
                match ipc_client::send_pause() {
                    Ok(()) => {
                        self.paused = true;
                        self.log_msg("Paused".into());
                    }
                    Err(e) => self.log_msg(format!("Pause failed: {}", e)),
                }
            }

            Message::ResumeClicked => {
                match ipc_client::send_resume() {
                    Ok(()) => {
                        self.paused = false;
                        self.log_msg("Resumed".into());
                    }
                    Err(e) => self.log_msg(format!("Resume failed: {}", e)),
                }
            }

            Message::ShutdownClicked => {
                match ipc_client::send_shutdown() {
                    Ok(()) => {
                        self.daemon_connected = false;
                        self.status_text = "Daemon shut down".into();
                        self.log_msg("Daemon shutdown sent".into());
                    }
                    Err(e) => self.log_msg(format!("Shutdown failed: {}", e)),
                }
            }

            Message::RefreshStatus | Message::Tick => {
                return Task::perform(
                    async { query_daemon_status() },
                    Message::StatusReceived,
                );
            }

            Message::StatusReceived(status) => {
                if status.starts_with('{') {
                    self.daemon_connected = true;
                    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&status) {
                        if let Some(fps) = parsed.get("fps").and_then(|v| v.as_f64()) {
                            self.fps = fps as f32;
                        }
                        if let Some(paused) = parsed.get("paused").and_then(|v| v.as_bool()) {
                            self.paused = paused;
                        }
                        if let Some(shade) = parsed.get("loaded_shade").and_then(|v| v.as_str()) {
                            self.loaded_shade = Some(shade.to_string());
                        }
                        self.status_text = format!("Connected | {:.1} FPS", self.fps);
                    }
                } else if status.contains("error") || status.contains("Could not") {
                    self.daemon_connected = false;
                    self.status_text = "Daemon not running".into();
                } else {
                    self.status_text = status;
                }
            }

            // Import flow
            Message::ImportPathChanged(s) => self.import_path = s,
            Message::ImportNameChanged(s) => self.import_name = s,
            Message::ImportAuthorChanged(s) => self.import_author = s,

            Message::BrowseImportClicked => {
                return Task::perform(
                    async {
                        let handle = rfd::AsyncFileDialog::new()
                            .set_title("Select Shadertoy GLSL file")
                            .add_filter("GLSL Shader", &["glsl", "frag", "txt"])
                            .pick_file()
                            .await;
                        handle.map(|f| f.path().to_path_buf())
                    },
                    Message::ImportFileSelected,
                );
            }

            Message::ImportFileSelected(Some(path)) => {
                self.import_path = path.to_string_lossy().to_string();
            }
            Message::ImportFileSelected(None) => {}

            Message::ImportClicked => {
                let path = self.import_path.clone();
                let name = self.import_name.clone();
                let author = self.import_author.clone();
                return Task::perform(
                    async move { do_import(&path, &name, &author) },
                    Message::ImportResult,
                );
            }

            Message::ImportResult(msg) => {
                self.import_result = msg.clone();
                self.log_msg(msg);
            }

            // Shadertoy download flow
            Message::ShadertoyUrlChanged(s) => self.shadertoy_url = s,

            Message::DownloadShadertoyClicked => {
                if self.shadertoy_url.is_empty() {
                    self.download_result = "Enter a Shadertoy URL or shader ID".into();
                } else {
                    self.downloading = true;
                    self.download_result = "Downloading...".into();
                    let url = self.shadertoy_url.clone();
                    return Task::perform(
                        async move { do_download_shadertoy(&url).await },
                        Message::DownloadResult,
                    );
                }
            }

            Message::DownloadResult(msg) => {
                self.downloading = false;
                self.download_result = msg.clone();
                self.log_msg(msg);
            }
        }

        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        let title = text("Kroma Wallpaper Engine").size(24);

        // Status bar
        let status_dot = if self.daemon_connected { "● " } else { "○ " };
        let status_line = text(format!("{}{}", status_dot, self.status_text)).size(14);

        // --- Controls ---
        let pause_resume = if self.paused {
            button("Resume").on_press(Message::ResumeClicked)
        } else {
            button("Pause").on_press(Message::PauseClicked)
        };

        let controls = row![
            button("Load Shade").on_press(Message::LoadShadeClicked),
            pause_resume,
            button("Refresh").on_press(Message::RefreshStatus),
            button("Shutdown Daemon").on_press(Message::ShutdownClicked),
        ]
        .spacing(8);

        // --- Loaded shade info ---
        let shade_info = if let Some(ref shade) = self.loaded_shade {
            let short_name = shade.rsplit('/').next().unwrap_or(shade);
            row![
                text(format!("Active: {}", short_name)).size(14),
                button("Unload").on_press(Message::UnloadShadeClicked),
            ]
            .spacing(8)
        } else {
            row![text("No shade loaded").size(14)]
        };

        // --- Import section ---
        let import_section = column![
            text("Import Shadertoy Shader").size(18),
            row![
                text_input("Path to .glsl file...", &self.import_path)
                    .on_input(Message::ImportPathChanged)
                    .width(Length::Fill),
                button("Browse").on_press(Message::BrowseImportClicked),
            ]
            .spacing(8),
            row![
                text_input("Shader name", &self.import_name)
                    .on_input(Message::ImportNameChanged),
                text_input("Author", &self.import_author)
                    .on_input(Message::ImportAuthorChanged),
                button("Import").on_press(Message::ImportClicked),
            ]
            .spacing(8),
            text(&self.import_result).size(12),
        ]
        .spacing(6);

        // --- Shadertoy Download section ---
        let download_btn = if self.downloading {
            button("Downloading...")
        } else {
            button("Download").on_press(Message::DownloadShadertoyClicked)
        };

        let download_section = column![
            text("Download from Shadertoy").size(18),
            row![
                text_input("Shadertoy URL or ID (e.g. XsXXDn)", &self.shadertoy_url)
                    .on_input(Message::ShadertoyUrlChanged)
                    .width(Length::Fill),
                download_btn,
            ]
            .spacing(8),
            text(&self.download_result).size(12),
        ]
        .spacing(6);

        // --- Log section ---
        let log_content: Element<Message> = if self.log_messages.is_empty() {
            text("No events yet.").size(12).into()
        } else {
            let items: Vec<Element<Message>> = self
                .log_messages
                .iter()
                .rev()
                .take(20)
                .map(|m| text(m).size(12).into())
                .collect();
            column(items).spacing(2).into()
        };

        let log_section = column![
            text("Event Log").size(18),
            scrollable(log_content).height(Length::Fixed(150.0)),
        ]
        .spacing(4);

        // --- Assemble ---
        let content = column![
            title,
            status_line,
            horizontal_rule(1),
            controls,
            shade_info,
            horizontal_rule(1),
            import_section,
            horizontal_rule(1),
            download_section,
            horizontal_rule(1),
            log_section,
        ]
        .spacing(12)
        .padding(20)
        .width(Length::Fill);

        container(content)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    fn log_msg(&mut self, msg: String) {
        let timestamp = chrono_now();
        self.log_messages.push(format!("[{}] {}", timestamp, msg));
        if self.log_messages.len() > 100 {
            self.log_messages.remove(0);
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn query_daemon_status() -> String {
    match ipc_client::query_status() {
        Ok(s) => s,
        Err(e) => format!("Could not connect: {}", e),
    }
}

fn do_import(path: &str, name: &str, author: &str) -> String {
    let p = PathBuf::from(path);
    if !p.exists() {
        return format!("File not found: {}", p.display());
    }
    match importer::import_shadertoy_file(&p, name, author) {
        Ok(out) => format!("Created: {}", out.display()),
        Err(e) => format!("Import failed: {}", e),
    }
}

async fn do_download_shadertoy(url_or_id: &str) -> String {
    // Default output directory: ~/.local/share/kroma/shaders/
    let output_dir = dirs_output_dir();
    match importer::download_shadertoy(url_or_id, &output_dir, None).await {
        Ok((shade_path, name)) => {
            format!("Downloaded '{}': {}", name, shade_path.display())
        }
        Err(e) => format!("Download failed: {}", e),
    }
}

fn dirs_output_dir() -> PathBuf {
    if let Some(data) = std::env::var_os("XDG_DATA_HOME") {
        PathBuf::from(data).join("kroma").join("shaders")
    } else if let Some(home) = std::env::var_os("HOME") {
        PathBuf::from(home)
            .join(".local")
            .join("share")
            .join("kroma")
            .join("shaders")
    } else {
        PathBuf::from("./shaders")
    }
}

fn chrono_now() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs() % 86400;
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    format!("{:02}:{:02}:{:02}", h, m, s)
}
