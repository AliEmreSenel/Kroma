//! Portable media packages with eagerly available posters and lazy video previews.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command as Process;

use anyhow::{Context, Result, bail, ensure};
use clap::Subcommand;
use kroma_shared::shade::{CompressionPolicy, LiveShadePackage};
use kroma_shared::types::{ShadeConfig, TextureType};
use serde::{Deserialize, Serialize};

/// Media-library operations shared by desktop choosers and manual imports.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Embed original media without re-encoding; generate a poster and small hover video.
    Import {
        input: PathBuf,
        #[arg(short, long)]
        output: Option<PathBuf>,
        #[arg(long)]
        name: Option<String>,
        /// Existing still preview, instead of generating one with FFmpeg.
        #[arg(long)]
        thumbnail: Option<PathBuf>,
        /// Existing MP4/WebM hover preview, including for shader packages.
        #[arg(long)]
        video_preview: Option<PathBuf>,
        /// Generate media-only previews with FFmpeg instead of Kroma's renderer.
        #[arg(long)]
        media_preview: bool,
        /// Keep a video package poster-only.
        #[arg(long)]
        no_video_preview: bool,
        /// Replace an existing output atomically.
        #[arg(long)]
        force: bool,
    },
    /// Render genuine package previews using an isolated Kroma daemon.
    RenderPreview {
        package: PathBuf,
        #[arg(long)]
        thumbnail: Option<PathBuf>,
        #[arg(long)]
        video: Option<PathBuf>,
        /// Store rendered previews in the package atomically.
        #[arg(long)]
        embed: bool,
        #[arg(long, default_value_t = 640)]
        width: u32,
        #[arg(long, default_value_t = 360)]
        height: u32,
        #[arg(long, default_value_t = 12)]
        fps: u32,
        #[arg(long, default_value_t = 3.0)]
        seconds: f64,
        /// Full wallpaper canvas before downsampling the final composited output.
        #[arg(long, default_value_t = 1920)]
        render_width: u32,
        #[arg(long, default_value_t = 1080)]
        render_height: u32,
        #[arg(long)]
        daemon: Option<PathBuf>,
    },
    /// Print small package metadata as JSON without extracting original media.
    Info { package: PathBuf },
    /// Extract embedded previews atomically for a chooser cache.
    Preview {
        package: PathBuf,
        #[arg(long, required_unless_present = "video")]
        thumbnail: Option<PathBuf>,
        #[arg(long)]
        video: Option<PathBuf>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MediaMetadata {
    version: u32,
    kind: String,
    source_name: String,
    width: u32,
    height: u32,
    fps: f64,
    duration: f64,
    codec: String,
    original_bytes: u64,
}

#[derive(Debug, Serialize)]
struct Info {
    name: String,
    kind: String,
    source_name: String,
    width: u32,
    height: u32,
    fps: f64,
    duration: f64,
    codec: String,
    filesize: u64,
    has_thumbnail: bool,
    video_preview: Option<String>,
    preview_renderer: Option<String>,
}

/// Execute an operation without connecting to or changing the running daemon.
pub fn run(command: Command) -> Result<()> {
    match command {
        Command::Import {
            input,
            output,
            name,
            thumbnail,
            video_preview,
            media_preview,
            no_video_preview,
            force,
        } => {
            let output = output.unwrap_or_else(|| input.with_extension("shade"));
            ensure!(
                force || !output.exists(),
                "Output already exists: {}",
                output.display()
            );
            ensure!(
                input.canonicalize()? != absolute_output(&output)?,
                "Input and output must differ"
            );
            let metadata = probe(&input)?;
            let name = name.unwrap_or_else(|| {
                input
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            });
            let source = format!("assets/{}", metadata.source_name);
            let mut config: ShadeConfig =
                toml::from_str("[meta]\nname='Media'\nauthor='Kroma'\n[states.active]\nlength=0")?;
            config.meta.name = name;
            let definition = toml::from_str(&format!(
                "type={}\nsource={}\nlooping=true\nbinding=0",
                toml::Value::String(
                    if metadata.kind == "video" {
                        "video"
                    } else {
                        "image"
                    }
                    .into()
                ),
                toml::Value::String(source.clone())
            ))?;
            config
                .states
                .active
                .as_mut()
                .expect("active state")
                .textures
                .insert("iChannel0".into(), definition);
            let mut package = LiveShadePackage::new_empty(config);
            package.add_asset(source.clone(), std::fs::read(&input)?);
            package.set_entry_compression(source, CompressionPolicy::None);
            package.add_asset("media.json".into(), serde_json::to_vec(&metadata)?);
            let supplied_preview = thumbnail.is_some() || video_preview.is_some();
            let generate_video =
                metadata.kind == "video" && !no_video_preview && video_preview.is_none();
            let mut rendered_poster = None;
            let mut rendered_video = None;
            if !media_preview && (thumbnail.is_none() || generate_video) {
                let directory = tempfile::tempdir()?;
                let staged = directory.path().join("media.shade");
                package.save(&staged)?;
                let rendered = render_native(&staged, generate_video, &RenderOptions::default())?;
                rendered_poster = Some(rendered.0);
                rendered_video = rendered.1;
            }
            package.preview = Some(match thumbnail {
                Some(path) => std::fs::read(path)?,
                None => match rendered_poster {
                    Some(bytes) => bytes,
                    None => still_preview(&input)?,
                },
            });
            if let Some(path) = video_preview {
                let extension = path.extension().and_then(|v| v.to_str()).unwrap_or("");
                ensure!(
                    matches!(extension, "mp4" | "webm"),
                    "Video preview must be MP4 or WebM"
                );
                package.add_asset(format!("preview.{extension}"), std::fs::read(path)?);
            } else if generate_video {
                package.add_asset(
                    "preview.mp4".into(),
                    match rendered_video {
                        Some(bytes) => bytes,
                        None => animated_preview(&input)?,
                    },
                );
            }
            package.add_asset("preview.json".into(), serde_json::to_vec(&serde_json::json!({"renderer":if supplied_preview {"provided"} else if media_preview {"media"} else {"kroma"},"width":640,"height":360,"fps":12}))?);
            save_package(&package, &output, force)?;
            println!("{}", output.display());
        }
        Command::RenderPreview {
            package,
            thumbnail,
            video,
            embed,
            width,
            height,
            fps,
            seconds,
            render_width,
            render_height,
            daemon,
        } => {
            ensure!(
                embed || thumbnail.is_some() || video.is_some(),
                "Choose --thumbnail, --video, or --embed"
            );
            let mut loaded = LiveShadePackage::load(&package)?;
            let generate_video =
                video.is_some() || (embed && info(&loaded, &package)?.kind != "static");
            let options = RenderOptions {
                width,
                height,
                fps,
                seconds,
                render_width,
                render_height,
                daemon,
            };
            let (poster, clip) = render_native(&package, generate_video, &options)?;
            if let Some(output) = thumbnail {
                ensure!(
                    absolute_output(&output)? != package.canonicalize()?,
                    "Preview cannot replace package"
                );
                atomic_write(&output, &poster)?;
            }
            if let (Some(output), Some(bytes)) = (video, clip.as_ref()) {
                ensure!(
                    absolute_output(&output)? != package.canonicalize()?,
                    "Preview cannot replace package"
                );
                atomic_write(&output, bytes)?;
            }
            if embed {
                loaded.preview = Some(poster);
                if let Some(bytes) = clip {
                    loaded.remove_asset("preview.webm");
                    loaded.add_asset("preview.mp4".into(), bytes);
                }
                loaded.add_asset("preview.json".into(), serde_json::to_vec(&serde_json::json!({"renderer":"kroma","width":width,"height":height,"fps":fps,"render_width":render_width,"render_height":render_height}))?);
                save_package(&loaded, &package, true)?;
            }
        }
        Command::Info { package } => {
            let loaded = LiveShadePackage::load(&package)?;
            println!("{}", serde_json::to_string(&info(&loaded, &package)?)?);
        }
        Command::Preview {
            package,
            thumbnail,
            video,
        } => {
            let loaded = LiveShadePackage::load(&package)?;
            if let Some(output) = &thumbnail {
                ensure!(
                    absolute_output(output)? != package.canonicalize()?,
                    "Preview output cannot replace the package"
                );
                let bytes = loaded
                    .preview
                    .as_deref()
                    .context("Package has no still thumbnail")?;
                atomic_write(output, bytes)?;
            }
            if let Some(output) = &video {
                ensure!(
                    absolute_output(output)? != package.canonicalize()?,
                    "Preview output cannot replace the package"
                );
                let entry = video_entry(&loaded).context("Package has no video preview")?;
                // Only this explicit request reads video-preview bytes.
                let bytes = loaded
                    .read_entry_bytes(&entry)?
                    .context("Missing preview entry")?;
                atomic_write(output, &bytes)?;
            }
        }
    }
    Ok(())
}

struct RenderOptions {
    width: u32,
    height: u32,
    fps: u32,
    seconds: f64,
    render_width: u32,
    render_height: u32,
    daemon: Option<PathBuf>,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            width: 640,
            height: 360,
            fps: 12,
            seconds: 3.0,
            render_width: 1920,
            render_height: 1080,
            daemon: None,
        }
    }
}

fn render_native(
    package: &Path,
    video: bool,
    options: &RenderOptions,
) -> Result<(Vec<u8>, Option<Vec<u8>>)> {
    ensure!(
        options.width > 0 && options.height > 0 && options.width <= 4096 && options.height <= 4096,
        "Invalid preview dimensions"
    );
    ensure!(
        options.render_width > 0
            && options.render_height > 0
            && options.render_width <= 8192
            && options.render_height <= 8192,
        "Invalid canvas dimensions"
    );
    ensure!(
        options.fps > 0
            && options.fps <= 60
            && options.seconds.is_finite()
            && options.seconds > 0.0
            && options.seconds <= 30.0,
        "Invalid preview duration/FPS"
    );
    let directory = tempfile::tempdir()?;
    let root = directory.path();
    for child in ["config/kroma", "cache", "run"] {
        std::fs::create_dir_all(root.join(child))?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(root.join("run"), std::fs::Permissions::from_mode(0o700))?;
    }
    let config = format!(
        "current_shade={}\ntarget_fps=60\ngpu_power='low'\n[runtime]\npersist_current_shade=false\n",
        toml::Value::String(package.canonicalize()?.to_string_lossy().into_owned())
    );
    std::fs::write(root.join("config/kroma/config.toml"), config)?;
    let poster = root.join("poster.jpg");
    let clip = root.join("preview.mp4");
    let job = serde_json::json!({"thumbnail":poster,"video":if video {Some(&clip)}else{None},"width":options.width,"height":options.height,"fps":options.fps,"seconds":options.seconds});
    let job_path = root.join("job.json");
    std::fs::write(&job_path, serde_json::to_vec(&job)?)?;
    let daemon = options
        .daemon
        .clone()
        .or_else(|| std::env::var_os("KROMA_DAEMON").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("kroma-daemon"));
    let log_path = root.join("render.log");
    let mut child = Process::new(daemon)
        .arg("--render-preview")
        .arg(job_path)
        .env("KROMA_PREVIEW_JOB", "1")
        .env("XDG_CURRENT_DESKTOP", "KDE")
        .env("XDG_SESSION_TYPE", "wayland")
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env("XDG_RUNTIME_DIR", root.join("run"))
        .env(
            "KROMA_HEADLESS_SIZE",
            format!("{}x{}", options.render_width, options.render_height),
        )
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("DISPLAY")
        .env_remove("HYPRLAND_INSTANCE_SIGNATURE")
        .stdout(std::process::Stdio::null())
        .stderr(std::fs::File::create(&log_path)?)
        .spawn()
        .context("Start Kroma preview renderer (set KROMA_DAEMON to the updated daemon)")?;
    let deadline = std::time::Instant::now()
        + std::time::Duration::from_secs(60 + options.seconds.ceil() as u64);
    loop {
        if let Some(status) = child.try_wait()? {
            ensure!(
                status.success(),
                "Kroma preview rendering failed: {}",
                std::fs::read_to_string(&log_path).unwrap_or_default()
            );
            break;
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("Kroma preview renderer timed out; ensure the updated daemon is installed");
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    Ok((
        std::fs::read(poster)?,
        if video {
            Some(std::fs::read(clip)?)
        } else {
            None
        },
    ))
}

fn absolute_output(path: &Path) -> Result<PathBuf> {
    if path.exists() {
        return Ok(path.canonicalize()?);
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent)?;
    Ok(parent
        .canonicalize()?
        .join(path.file_name().context("Output has no filename")?))
}

fn save_package(package: &LiveShadePackage, output: &Path, replace: bool) -> Result<()> {
    let output = absolute_output(output)?;
    let temporary =
        tempfile::NamedTempFile::new_in(output.parent().context("Output has no parent")?)?;
    package.save(temporary.path())?;
    if replace {
        temporary.persist(output).map_err(|e| e.error)?;
    } else {
        temporary.persist_noclobber(output).map_err(|e| e.error)?;
    }
    Ok(())
}

fn atomic_write(output: &Path, bytes: &[u8]) -> Result<()> {
    let output = absolute_output(output)?;
    let mut temporary =
        tempfile::NamedTempFile::new_in(output.parent().context("Output has no parent")?)?;
    temporary.write_all(bytes)?;
    temporary.persist(output).map_err(|e| e.error)?;
    Ok(())
}

fn video_entry(package: &LiveShadePackage) -> Option<String> {
    // Inspect names only: preview videos must not be decoded by info or playback.
    package
        .asset_entries()
        .into_iter()
        .find(|entry| matches!(entry.name.as_str(), "preview.mp4" | "preview.webm"))
        .map(|entry| entry.name)
}

fn info(package: &LiveShadePackage, path: &Path) -> Result<Info> {
    let metadata = package
        .read_entry_bytes("media.json")?
        .map(|bytes| serde_json::from_slice::<MediaMetadata>(&bytes))
        .transpose()?;
    let states = [
        &package.config.states.load,
        &package.config.states.active,
        &package.config.states.unload,
    ];
    let composed = package
        .config
        .states
        .load
        .as_ref()
        .is_some_and(|state| state.length > 0.0)
        || package
            .config
            .states
            .unload
            .as_ref()
            .is_some_and(|state| state.length > 0.0)
        || states
            .iter()
            .filter_map(|state| state.as_ref())
            .any(|state| {
                state.shader.is_some()
                    || !state.buffers.is_empty()
                    || state.textures.values().any(|texture| {
                        matches!(
                            texture.ty,
                            TextureType::Shader
                                | TextureType::Slideshow
                                | TextureType::AudioSpectrum
                        )
                    })
            });
    let video = states
        .iter()
        .filter_map(|state| state.as_ref())
        .any(|state| {
            state
                .textures
                .values()
                .any(|texture| texture.ty == TextureType::Video)
        });
    let kind = if composed {
        "shader"
    } else if video {
        "video"
    } else {
        "static"
    }
    .to_string();
    Ok(Info {
        name: package.config.meta.name.clone(),
        kind,
        source_name: metadata
            .as_ref()
            .map(|m| m.source_name.clone())
            .unwrap_or_default(),
        width: metadata.as_ref().map(|m| m.width).unwrap_or(0),
        height: metadata.as_ref().map(|m| m.height).unwrap_or(0),
        fps: metadata.as_ref().map(|m| m.fps).unwrap_or(0.0),
        duration: metadata.as_ref().map(|m| m.duration).unwrap_or(0.0),
        codec: metadata
            .as_ref()
            .map(|m| m.codec.clone())
            .unwrap_or_default(),
        filesize: std::fs::metadata(path)?.len(),
        has_thumbnail: package.preview.is_some(),
        video_preview: video_entry(package),
        preview_renderer: package
            .read_entry_bytes("preview.json")?
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .and_then(|value| value["renderer"].as_str().map(str::to_owned)),
    })
}

fn checked_output(command: &mut Process) -> Result<Vec<u8>> {
    let output = command
        .output()
        .context("Run FFmpeg/ffprobe (required only when importing media)")?;
    if !output.status.success() {
        bail!(
            "Media tool failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(output.stdout)
}

fn probe(input: &Path) -> Result<MediaMetadata> {
    let data = checked_output(
        Process::new("ffprobe")
            .args([
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-show_entries",
                "stream=width,height,r_frame_rate,codec_name:format=duration",
                "-of",
                "json",
            ])
            .arg(input),
    )?;
    let value: serde_json::Value = serde_json::from_slice(&data)?;
    let stream = value["streams"]
        .get(0)
        .context("Input has no video/image stream")?;
    let extension = input
        .extension()
        .and_then(|v| v.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let kind = if matches!(extension.as_str(), "jpg" | "jpeg" | "png" | "webp") {
        "static"
    } else {
        "video"
    };
    let fps = stream["r_frame_rate"]
        .as_str()
        .and_then(|v| v.split_once('/'))
        .and_then(|(a, b)| Some(a.parse::<f64>().ok()? / b.parse::<f64>().ok()?))
        .filter(|v| v.is_finite() && *v > 0.0)
        .unwrap_or(0.0);
    Ok(MediaMetadata {
        version: 1,
        kind: kind.into(),
        source_name: input
            .file_name()
            .context("Missing filename")?
            .to_string_lossy()
            .into_owned(),
        width: stream["width"].as_u64().unwrap_or(0) as u32,
        height: stream["height"].as_u64().unwrap_or(0) as u32,
        fps,
        duration: value["format"]["duration"]
            .as_str()
            .and_then(|v| v.parse::<f64>().ok())
            .filter(|v| v.is_finite())
            .unwrap_or(0.0),
        codec: stream["codec_name"].as_str().unwrap_or("").into(),
        original_bytes: std::fs::metadata(input)?.len(),
    })
}

fn still_preview(input: &Path) -> Result<Vec<u8>> {
    checked_output(
        Process::new("ffmpeg")
            .args(["-nostdin", "-v", "error", "-threads", "1", "-i"])
            .arg(input)
            .args([
                "-vf",
                "scale=640:360:force_original_aspect_ratio=decrease",
                "-frames:v",
                "1",
                "-threads",
                "1",
                "-q:v",
                "3",
                "-f",
                "image2pipe",
                "-c:v",
                "mjpeg",
                "pipe:1",
            ]),
    )
}

fn animated_preview(input: &Path) -> Result<Vec<u8>> {
    checked_output(
        Process::new("ffmpeg")
            .args(["-nostdin", "-v", "error", "-threads", "1", "-i"])
            .arg(input)
            .args([
                "-t",
                "3",
                "-an",
                "-vf",
                "fps=12,scale=640:360:force_original_aspect_ratio=decrease:force_divisible_by=2",
                "-c:v",
                "libx264",
                "-preset",
                "veryfast",
                "-crf",
                "28",
                "-threads",
                "1",
                "-pix_fmt",
                "yuv420p",
                "-movflags",
                "frag_keyframe+empty_moov",
                "-f",
                "mp4",
                "pipe:1",
            ]),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_video_is_optional_lazy_and_roundtrips_without_runtime_textures() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("example.shade");
        let config =
            toml::from_str("[meta]\nname='Preview'\nauthor='test'\n[states.active]\nlength=0")
                .unwrap();
        let mut package = LiveShadePackage::new_empty(config);
        package.preview = Some(vec![1, 2, 3]);
        package.add_asset("preview.mp4".into(), vec![5; 128 * 1024]);
        save_package(&package, &path, false).unwrap();
        let loaded = LiveShadePackage::load(&path).unwrap();
        let information = info(&loaded, &path).unwrap();
        assert!(information.has_thumbnail);
        assert_eq!(information.video_preview.as_deref(), Some("preview.mp4"));
        assert!(
            loaded
                .config
                .states
                .active
                .as_ref()
                .unwrap()
                .textures
                .is_empty()
        );
        let image = directory.path().join("poster.jpg");
        let video = directory.path().join("hover.mp4");
        run(Command::Preview {
            package: path.clone(),
            thumbnail: Some(image.clone()),
            video: Some(video.clone()),
        })
        .unwrap();
        assert_eq!(std::fs::read(image).unwrap(), vec![1, 2, 3]);
        assert_eq!(std::fs::read(video).unwrap(), vec![5; 128 * 1024]);
        assert!(save_package(&package, &path, false).is_err());
        assert!(
            run(Command::Preview {
                package: path.clone(),
                thumbnail: Some(path.clone()),
                video: None
            })
            .is_err()
        );
        assert!(LiveShadePackage::load(&path).is_ok());
    }
}
