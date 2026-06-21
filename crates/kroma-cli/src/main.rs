//! Kroma CLI — command-line interface for the Kroma wallpaper engine.
//!
//! Controls the daemon, imports Shadertoy shaders, and manages .shade packages.

mod importer;
mod inspect;
mod ipc_client;

use std::io;
use std::path::PathBuf;

use anyhow::Result;
use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::{Shell, generate};
use inspect::InspectCommand;
use log::info;

/// Kroma — a high-performance wallpaper engine for Linux.
#[derive(Parser)]
#[command(name = "kroma", version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Load a .shade package on the daemon.
    Load {
        /// Path to the .shade package file.
        path: PathBuf,
        /// Force immediate load by preempting current lifecycle.
        #[arg(long)]
        force: bool,
        /// Optional transition usage in `scope.id[:seconds]` format.
        #[arg(long)]
        transition: Option<String>,
    },

    /// Unload current shade and transition to terminal/no-shade state.
    Unload,

    /// Pause wallpaper rendering.
    Pause,

    /// Resume wallpaper rendering.
    Resume,

    /// Shut down the daemon gracefully.
    Shutdown,

    /// Query the daemon's current status.
    Status,

    /// Reload the currently loaded shade package.
    Reload,

    /// Import a Shadertoy GLSL file into a .shade package.
    Import {
        /// Path to the Shadertoy GLSL shader file.
        shader: PathBuf,

        /// Name for the shade package.
        #[arg(short, long, default_value = "Imported Shader")]
        name: String,

        /// Author name.
        #[arg(short, long, default_value = "Unknown")]
        author: String,
    },

    /// Download a shader from Shadertoy and produce a .shade package.
    Download {
        /// Shadertoy URL or shader ID.
        url_or_id: String,

        /// Output directory for the .shade and .glsl files.
        #[arg(short, long)]
        output_dir: Option<PathBuf>,
    },

    /// Convert a folder into a .shade package.
    Pack {
        /// Input folder that contains config.toml and optional assets.
        folder: PathBuf,

        /// Output .shade package path.
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Default compression codec for packed entries.
        /// Use `auto` to keep built-in type defaults.
        #[arg(long, default_value = "auto")]
        codec: String,

        /// Default compression level (integer) for zstd, or `auto`.
        #[arg(long, default_value = "auto")]
        level: String,

        /// Per-entry compression override: PATH=CODEC[:LEVEL]
        /// Example: --entry-compression assets/video.mp4=none
        /// Example: --entry-compression shader.frag=zstd:10
        #[arg(long = "entry-compression")]
        entry_compression: Vec<String>,
    },

    /// Inspect a v2 .shade package in detail.
    Inspect {
        /// Path to the .shade package file.
        path: PathBuf,

        /// Inspect subcommand (defaults to summary when omitted).
        #[command(subcommand)]
        command: Option<InspectCommand>,
    },

    /// Generate shell completions for the given shell.
    Completions {
        /// The shell to generate completions for.
        shell: Shell,
    },
}

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Load {
            path,
            force,
            transition,
        } => cmd_load(path, force, transition)?,
        Commands::Unload => cmd_unload()?,
        Commands::Pause => cmd_pause()?,
        Commands::Resume => cmd_resume()?,
        Commands::Shutdown => cmd_shutdown()?,
        Commands::Status => cmd_status()?,
        Commands::Reload => cmd_reload()?,
        Commands::Import {
            shader,
            name,
            author,
        } => cmd_import(shader, &name, &author)?,
        Commands::Download {
            url_or_id,
            output_dir,
        } => cmd_download(&url_or_id, output_dir)?,
        Commands::Pack {
            folder,
            output,
            codec,
            level,
            entry_compression,
        } => cmd_pack(folder, output, codec, level, entry_compression)?,
        Commands::Inspect { path, command } => inspect::run(path, command)?,
        Commands::Completions { shell } => {
            let mut cmd = Cli::command();
            generate(shell, &mut cmd, "kroma", &mut io::stdout());
        }
    }

    Ok(())
}

/// Load a .shade package — resolves to an absolute path before sending to the daemon.
fn cmd_load(path: PathBuf, force: bool, transition: Option<String>) -> Result<()> {
    let absolute = std::fs::canonicalize(&path).map_err(|e| {
        anyhow::anyhow!(
            "Cannot resolve path '{}': {}. Does the file exist?",
            path.display(),
            e
        )
    })?;

    let path_str = absolute.to_string_lossy();
    ipc_client::send_load(&path_str, force, transition.as_deref())?;
    info!(
        "Sent load command to daemon: {} (force={}, transition={:?})",
        path_str, force, transition
    );
    Ok(())
}

fn cmd_unload() -> Result<()> {
    ipc_client::send_unload()?;
    info!("Sent unload command");
    Ok(())
}

fn cmd_pause() -> Result<()> {
    ipc_client::send_pause()?;
    info!("Sent pause command");
    Ok(())
}

fn cmd_resume() -> Result<()> {
    ipc_client::send_resume()?;
    info!("Sent resume command");
    Ok(())
}

fn cmd_shutdown() -> Result<()> {
    ipc_client::send_shutdown()?;
    info!("Sent shutdown command");
    Ok(())
}

fn cmd_status() -> Result<()> {
    match ipc_client::query_status() {
        Ok(response) => println!("{}", response),
        Err(e) => eprintln!("Failed: {}", e),
    }
    Ok(())
}

fn cmd_reload() -> Result<()> {
    ipc_client::send_reload()?;
    info!("Sent reload command");
    Ok(())
}

fn cmd_import(shader: PathBuf, name: &str, author: &str) -> Result<()> {
    info!("Importing shader from: {}", shader.display());
    let output = importer::import_shadertoy_file(&shader, name, author)?;
    info!("Created shade package: {}", output.display());
    Ok(())
}

fn cmd_download(url_or_id: &str, output_dir: Option<PathBuf>) -> Result<()> {
    let dir = output_dir.unwrap_or_else(default_output_dir);
    let rt = tokio::runtime::Runtime::new()?;
    match rt.block_on(importer::download_shadertoy(url_or_id, &dir, None)) {
        Ok((shade_path, name)) => {
            info!("Downloaded '{}': {}", name, shade_path.display());
        }
        Err(e) => eprintln!("Download failed: {}", e),
    }
    Ok(())
}

fn cmd_pack(
    folder: PathBuf,
    output: Option<PathBuf>,
    codec: String,
    level: String,
    entry_compression: Vec<String>,
) -> Result<()> {
    let options = importer::PackOptions {
        default_codec: codec,
        default_level: level,
        entry_compression,
    };
    let shade_path = importer::pack_folder_to_shade(&folder, output.as_deref(), &options)?;
    info!("Packed folder into: {}", shade_path.display());
    Ok(())
}

/// Default output directory for downloaded shaders.
fn default_output_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("XDG_DATA_HOME") {
        PathBuf::from(d).join("kroma/shaders")
    } else if let Some(h) = std::env::var_os("HOME") {
        PathBuf::from(h).join(".local/share/kroma/shaders")
    } else {
        PathBuf::from("./shaders")
    }
}

#[cfg(test)]
mod tests {
    use super::{Cli, Commands};
    use clap::Parser;

    #[test]
    fn parse_load_transition_option() {
        let cli = Cli::try_parse_from([
            "kroma",
            "load",
            "/tmp/demo.shade",
            "--force",
            "--transition",
            "kroma.fade:0.75",
        ])
        .expect("CLI parse should succeed");

        match cli.command {
            Commands::Load {
                path,
                force,
                transition,
            } => {
                assert_eq!(path.to_string_lossy(), "/tmp/demo.shade");
                assert!(force);
                assert_eq!(transition.as_deref(), Some("kroma.fade:0.75"));
            }
            _ => panic!("expected load command"),
        }
    }

    #[test]
    fn parse_load_without_transition_option() {
        let cli = Cli::try_parse_from(["kroma", "load", "/tmp/demo.shade"])
            .expect("CLI parse should succeed");

        match cli.command {
            Commands::Load {
                force, transition, ..
            } => {
                assert!(!force);
                assert!(transition.is_none());
            }
            _ => panic!("expected load command"),
        }
    }
}
