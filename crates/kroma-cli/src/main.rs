//! Kroma CLI — command-line interface for the Kroma wallpaper engine.
//!
//! Controls the daemon, imports Shadertoy shaders, and manages .shade packages.

mod importer;
mod ipc_client;

use std::io;
use std::path::PathBuf;

use anyhow::Result;
use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::{Shell, generate};
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
    },

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

    /// Convert a legacy ZIP-based .shade package to v2 .shade.
    Migrate {
        /// Input legacy .shade file.
        input: PathBuf,

        /// Output v2 .shade file.
        #[arg(short, long)]
        output: Option<PathBuf>,
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
        Commands::Load { path } => cmd_load(path)?,
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
        Commands::Migrate { input, output } => cmd_migrate(input, output)?,
        Commands::Completions { shell } => {
            let mut cmd = Cli::command();
            generate(shell, &mut cmd, "kroma", &mut io::stdout());
        }
    }

    Ok(())
}

/// Load a .shade package — resolves to an absolute path before sending to the daemon.
fn cmd_load(path: PathBuf) -> Result<()> {
    let absolute = std::fs::canonicalize(&path).map_err(|e| {
        anyhow::anyhow!(
            "Cannot resolve path '{}': {}. Does the file exist?",
            path.display(),
            e
        )
    })?;

    let path_str = absolute.to_string_lossy();
    ipc_client::send_load(&path_str)?;
    info!("Sent load command to daemon: {}", path_str);
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

fn cmd_migrate(input: PathBuf, output: Option<PathBuf>) -> Result<()> {
    let (shade_path, report) =
        importer::migrate_legacy_zip_to_v2_with_report(&input, output.as_deref())?;
    info!(
        "Migration embedded {} asset file(s)",
        report.embedded_assets
    );
    if !report.warnings.is_empty() {
        info!(
            "Migration completed with {} warning(s)",
            report.warnings.len()
        );
        for warning in &report.warnings {
            log::warn!("{}", warning);
        }
    }
    info!("Migrated package to v2: {}", shade_path.display());
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
