use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Subcommand, ValueEnum};
use serde::Serialize;

use kroma_shared::shade::{
    ChunkDebugInfo, CompressionStats, LiveShadePackage, PackageMetadata, VerifyMode, VerifyReport,
};
use kroma_shared::types::{ShadeConfig, ShadeStateDef, ShadeTransitionsUsage};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum InspectOutputFormat {
    Text,
    Json,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum InspectVerifyMode {
    Fast,
    Checksum,
    Decode,
}

impl From<InspectVerifyMode> for VerifyMode {
    fn from(value: InspectVerifyMode) -> Self {
        match value {
            InspectVerifyMode::Fast => VerifyMode::Fast,
            InspectVerifyMode::Checksum => VerifyMode::Checksum,
            InspectVerifyMode::Decode => VerifyMode::Decode,
        }
    }
}

#[derive(Debug, Subcommand)]
pub enum InspectCommand {
    /// Show top-level summary for a v2 .shade package.
    Summary {
        #[arg(long, value_enum, default_value_t = InspectOutputFormat::Text)]
        format: InspectOutputFormat,
    },
    /// Verify package integrity.
    Verify {
        #[arg(long, value_enum, default_value_t = InspectVerifyMode::Checksum)]
        mode: InspectVerifyMode,
        #[arg(long, value_enum, default_value_t = InspectOutputFormat::Text)]
        format: InspectOutputFormat,
    },
    /// List all entries in the package.
    List {
        #[arg(long, value_enum, default_value_t = InspectOutputFormat::Text)]
        format: InspectOutputFormat,
    },
    /// Show chunk-level diagnostics.
    Chunks {
        #[arg(long, default_value_t = 1)]
        depth: u8,
        #[arg(long, value_enum, default_value_t = InspectOutputFormat::Text)]
        format: InspectOutputFormat,
    },
    /// Show compression and distribution statistics.
    Stats {
        #[arg(long, value_enum, default_value_t = InspectOutputFormat::Text)]
        format: InspectOutputFormat,
    },
    /// Dump bytes for a package entry.
    DumpEntry {
        entry_path: String,
        #[arg(long, default_value_t = 256)]
        max_bytes: usize,
        #[arg(long)]
        full: bool,
        #[arg(long, value_enum, default_value_t = InspectOutputFormat::Text)]
        format: InspectOutputFormat,
    },
    /// Dump bytes for one chunk of a package entry.
    DumpChunk {
        entry_path: String,
        chunk_index: u32,
        #[arg(long)]
        decoded: bool,
        #[arg(long, default_value_t = 256)]
        max_bytes: usize,
        #[arg(long)]
        full: bool,
        #[arg(long, value_enum, default_value_t = InspectOutputFormat::Text)]
        format: InspectOutputFormat,
    },
}

#[derive(Debug, Serialize)]
struct SummaryOutput {
    metadata: PackageMetadata,
    stats: CompressionStats,
    entry_count: usize,
    phases: Vec<PhaseOutput>,
    transitions_usage: TransitionUsageOutput,
    transitions: Vec<TransitionOutput>,
}

#[derive(Debug, Serialize)]
struct PhaseOutput {
    phase: String,
    length: f64,
    is_loop: bool,
    shader: Option<String>,
}

#[derive(Debug, Serialize)]
struct TransitionUsageOutput {
    on_load_to_active: Option<String>,
    on_active_to_unload: Option<String>,
}

#[derive(Debug, Serialize)]
struct TransitionOutput {
    id: String,
    shader: String,
    duration: f64,
    uniform_count: usize,
    texture_count: usize,
    buffer_count: usize,
}

#[derive(Debug, Serialize)]
struct DumpOutput {
    path: String,
    byte_len: usize,
    truncated: bool,
    preview_hex: String,
}

#[derive(Debug, Serialize)]
struct ChunkDumpOutput {
    path: String,
    chunk_index: u32,
    decoded: bool,
    chunk_info: ChunkDebugInfo,
    byte_len: usize,
    truncated: bool,
    preview_hex: String,
}

pub fn run(path: PathBuf, command: Option<InspectCommand>) -> Result<()> {
    let pkg = load_v2_package(&path)?;

    match command {
        None => cmd_summary(&pkg, InspectOutputFormat::Text),
        Some(InspectCommand::Summary { format }) => cmd_summary(&pkg, format),
        Some(InspectCommand::Verify { mode, format }) => cmd_verify(&pkg, mode, format),
        Some(InspectCommand::List { format }) => cmd_list(&pkg, format),
        Some(InspectCommand::Chunks { depth, format }) => cmd_chunks(&pkg, depth, format),
        Some(InspectCommand::Stats { format }) => cmd_stats(&pkg, format),
        Some(InspectCommand::DumpEntry {
            entry_path,
            max_bytes,
            full,
            format,
        }) => cmd_dump_entry(&pkg, &entry_path, max_bytes, full, format),
        Some(InspectCommand::DumpChunk {
            entry_path,
            chunk_index,
            decoded,
            max_bytes,
            full,
            format,
        }) => cmd_dump_chunk(
            &pkg,
            &entry_path,
            chunk_index,
            decoded,
            max_bytes,
            full,
            format,
        ),
    }
}

fn cmd_summary(pkg: &LiveShadePackage, format: InspectOutputFormat) -> Result<()> {
    let metadata = pkg.package_metadata()?;
    let stats = pkg.compression_stats()?;
    let entries = pkg.entry_debug_infos()?;
    let phases = collect_phase_outputs(&pkg.config);
    let transitions_usage = collect_transition_usage_output(&pkg.config.transitions_usage);
    let transitions = collect_transition_outputs(&pkg.config);

    match format {
        InspectOutputFormat::Json => {
            let out = SummaryOutput {
                metadata,
                stats,
                entry_count: entries.len(),
                phases,
                transitions_usage,
                transitions,
            };
            println!("{}", serde_json::to_string_pretty(&out)?);
        }
        InspectOutputFormat::Text => {
            println!("Kroma .shade Summary");
            println!("entries           : {}", metadata.entry_count);
            println!("chunks            : {}", metadata.total_chunks);
            println!("file size         : {}", format_bytes(metadata.file_size));
            println!(
                "index             : offset={} size={}",
                metadata.index_offset,
                format_bytes(metadata.index_size)
            );
            println!(
                "uncompressed total: {}",
                format_bytes(stats.total_uncompressed)
            );
            println!(
                "compressed total  : {}",
                format_bytes(stats.total_compressed)
            );
            println!("ratio             : {:.3}", stats.compression_ratio);
            println!(
                "savings           : {}",
                format_signed_bytes(stats.savings_vs_stored_bytes)
            );
            println!(
                "avg entropy       : {:.3} bits/byte",
                stats.average_chunk_entropy_bits_per_byte
            );
            if phases.is_empty() {
                println!("states            : none");
            } else {
                println!("states            : {}", phases.len());
                for phase in phases {
                    let shader = phase.shader.unwrap_or_else(|| "<none>".to_string());
                    if phase.is_loop {
                        println!(
                            "  - {:<8} loop_length={:.3}s shader={}",
                            phase.phase, phase.length, shader
                        );
                    } else {
                        println!(
                            "  - {:<8} length={:.3}s shader={}",
                            phase.phase, phase.length, shader
                        );
                    }
                }
            }

            println!(
                "transition usage  : load->active={} active->unload={}",
                transitions_usage
                    .on_load_to_active
                    .as_deref()
                    .unwrap_or("<none>"),
                transitions_usage
                    .on_active_to_unload
                    .as_deref()
                    .unwrap_or("<none>")
            );

            if transitions.is_empty() {
                println!("transitions       : none");
            } else {
                println!("transitions       : {}", transitions.len());
                for transition in transitions {
                    println!(
                        "  - {:<16} duration={:.3}s shader={} (u:{} t:{} b:{})",
                        transition.id,
                        transition.duration,
                        transition.shader,
                        transition.uniform_count,
                        transition.texture_count,
                        transition.buffer_count
                    );
                }
            }
        }
    }

    Ok(())
}

fn collect_transition_usage_output(usage: &ShadeTransitionsUsage) -> TransitionUsageOutput {
    TransitionUsageOutput {
        on_load_to_active: usage.on_load_to_active.clone(),
        on_active_to_unload: usage.on_active_to_unload.clone(),
    }
}

fn collect_transition_outputs(config: &ShadeConfig) -> Vec<TransitionOutput> {
    config
        .transitions
        .iter()
        .map(|(id, def)| TransitionOutput {
            id: id.clone(),
            shader: def.shader.clone(),
            duration: def.duration,
            uniform_count: def.uniforms.len(),
            texture_count: def.textures.len(),
            buffer_count: def.buffers.len(),
        })
        .collect()
}

fn cmd_verify(
    pkg: &LiveShadePackage,
    mode: InspectVerifyMode,
    format: InspectOutputFormat,
) -> Result<()> {
    let report = pkg.verify(mode.into())?;
    match format {
        InspectOutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        InspectOutputFormat::Text => {
            print_verify_report_text(&report);
        }
    }

    if report.ok {
        Ok(())
    } else {
        anyhow::bail!("verification failed: {} failure(s)", report.failures.len())
    }
}

fn cmd_list(pkg: &LiveShadePackage, format: InspectOutputFormat) -> Result<()> {
    let entries = pkg.entry_debug_infos()?;
    let phases = collect_phase_outputs(&pkg.config);

    match format {
        InspectOutputFormat::Json => {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "phases": phases,
                    "entries": entries,
                }))?
            );
        }
        InspectOutputFormat::Text => {
            if phases.is_empty() {
                println!("phases: none");
            } else {
                println!("phases:");
                for phase in phases {
                    let shader = phase.shader.unwrap_or_else(|| "<none>".to_string());
                    if phase.is_loop {
                        println!(
                            "  {:<8} loop_length={:.3}s shader={}",
                            phase.phase, phase.length, shader
                        );
                    } else {
                        println!(
                            "  {:<8} length={:.3}s shader={}",
                            phase.phase, phase.length, shader
                        );
                    }
                }
            }
            println!();
            println!(
                "{:<36} {:<12} {:>11} {:>11} {:>8} {:>8} {:>8} {:>18}",
                "path", "kind", "raw", "stored", "ratio", "chunks", "codec", "offset range"
            );
            for entry in entries {
                println!(
                    "{:<36} {:<12} {:>11} {:>11} {:>8.3} {:>8} {:>8} {:>18}",
                    truncate_for_table(&entry.path, 36),
                    format!("{:?}", entry.kind).to_ascii_lowercase(),
                    format_bytes(entry.uncompressed_size),
                    format_bytes(entry.compressed_size),
                    entry.compression_ratio,
                    entry.chunk_count,
                    format!("{:?}", entry.default_codec).to_ascii_lowercase(),
                    format!("{}-{}", entry.data_start_offset, entry.data_end_offset)
                );
            }
        }
    }

    Ok(())
}

fn collect_phase_outputs(config: &ShadeConfig) -> Vec<PhaseOutput> {
    let mut out = Vec::new();
    push_phase_output(&mut out, "load", config.states.load.as_ref());
    push_phase_output(&mut out, "active", config.states.active.as_ref());
    push_phase_output(&mut out, "unload", config.states.unload.as_ref());
    out
}

fn push_phase_output(out: &mut Vec<PhaseOutput>, phase: &str, state: Option<&ShadeStateDef>) {
    let Some(state) = state else {
        return;
    };
    out.push(PhaseOutput {
        phase: phase.to_string(),
        length: state.length,
        is_loop: phase == "active",
        shader: state.shader.clone(),
    });
}

fn cmd_chunks(pkg: &LiveShadePackage, depth: u8, format: InspectOutputFormat) -> Result<()> {
    if !(1..=4).contains(&depth) {
        anyhow::bail!("--depth must be between 1 and 4");
    }

    let entries = pkg.entry_debug_infos()?;

    match format {
        InspectOutputFormat::Json => {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "depth": depth,
                    "entries": entries,
                }))?
            );
        }
        InspectOutputFormat::Text => {
            for entry in entries {
                println!(
                    "{}  chunks={} raw={} stored={} ratio={:.3}",
                    entry.path,
                    entry.chunk_count,
                    format_bytes(entry.uncompressed_size),
                    format_bytes(entry.compressed_size),
                    entry.compression_ratio
                );

                if depth >= 2 {
                    for chunk in &entry.chunks {
                        print!(
                            "  [{:>4}] off={:<10} c={} u={} codec={:<4} crc={:08x}",
                            chunk.index,
                            chunk.data_offset,
                            format_bytes(chunk.compressed_size as u64),
                            format_bytes(chunk.uncompressed_size as u64),
                            format!("{:?}", chunk.codec).to_ascii_lowercase(),
                            chunk.checksum,
                        );

                        if depth >= 3 {
                            let per_chunk_ratio =
                                safe_ratio_u32(chunk.compressed_size, chunk.uncompressed_size);
                            print!(" ratio={:.3}", per_chunk_ratio);
                        }

                        if depth >= 4
                            && let Some(bytes) =
                                pkg.read_chunk_bytes(&entry.path, chunk.index as usize, false)?
                        {
                            let preview = hex_preview(&bytes, 24);
                            print!(" preview={}", preview);
                        }

                        println!();
                    }
                }
            }
        }
    }

    Ok(())
}

fn cmd_stats(pkg: &LiveShadePackage, format: InspectOutputFormat) -> Result<()> {
    let stats = pkg.compression_stats()?;

    match format {
        InspectOutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&stats)?);
        }
        InspectOutputFormat::Text => {
            println!("Compression Statistics");
            println!(
                "raw total      : {}",
                format_bytes(stats.total_uncompressed)
            );
            println!("stored total   : {}", format_bytes(stats.total_compressed));
            println!("ratio          : {:.3}", stats.compression_ratio);
            println!(
                "savings        : {}",
                format_signed_bytes(stats.savings_vs_stored_bytes)
            );
            println!(
                "avg entropy    : {:.3} bits/byte",
                stats.average_chunk_entropy_bits_per_byte
            );

            println!("\nBy codec:");
            for codec in &stats.by_codec {
                println!(
                    "  {:<4} chunks={:<6} stored={} raw={} ratio={:.3}",
                    format!("{:?}", codec.codec).to_ascii_lowercase(),
                    codec.chunk_count,
                    format_bytes(codec.compressed_size),
                    format_bytes(codec.uncompressed_size),
                    codec.ratio
                );
            }

            println!("\nLargest entries:");
            for entry in &stats.largest_entries {
                println!(
                    "  {:<36} raw={} stored={} ratio={:.3}",
                    truncate_for_table(&entry.path, 36),
                    format_bytes(entry.uncompressed_size),
                    format_bytes(entry.compressed_size),
                    entry.compression_ratio
                );
            }

            println!("\nChunk size distribution:");
            for bucket in &stats.chunk_size_distribution {
                println!("  {:<10} {}", bucket.label, bucket.chunk_count);
            }
        }
    }

    Ok(())
}

fn cmd_dump_entry(
    pkg: &LiveShadePackage,
    entry_path: &str,
    max_bytes: usize,
    full: bool,
    format: InspectOutputFormat,
) -> Result<()> {
    let bytes = pkg
        .read_entry_bytes(entry_path)?
        .with_context(|| format!("Entry '{}' not found", entry_path))?;

    let (preview, truncated) = select_preview(&bytes, max_bytes, full);
    let out = DumpOutput {
        path: entry_path.to_string(),
        byte_len: bytes.len(),
        truncated,
        preview_hex: hex_encode(preview),
    };

    match format {
        InspectOutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&out)?);
        }
        InspectOutputFormat::Text => {
            println!("entry      : {}", out.path);
            println!("size       : {}", format_bytes(out.byte_len as u64));
            println!("truncated  : {}", out.truncated);
            println!("hex        : {}", out.preview_hex);
            if let Ok(text) = std::str::from_utf8(preview) {
                println!("utf8 preview: {}", text.replace('\n', "\\n"));
            }
        }
    }

    Ok(())
}

fn cmd_dump_chunk(
    pkg: &LiveShadePackage,
    entry_path: &str,
    chunk_index: u32,
    decoded: bool,
    max_bytes: usize,
    full: bool,
    format: InspectOutputFormat,
) -> Result<()> {
    let chunks = pkg
        .chunk_debug_infos(entry_path)?
        .with_context(|| format!("Entry '{}' not found", entry_path))?;
    let chunk = chunks
        .iter()
        .find(|chunk| chunk.index == chunk_index)
        .cloned()
        .with_context(|| format!("Chunk {} not found in '{}'", chunk_index, entry_path))?;

    let bytes = pkg
        .read_chunk_bytes(entry_path, chunk_index as usize, decoded)?
        .with_context(|| format!("Chunk {} bytes are not available", chunk_index))?;
    let (preview, truncated) = select_preview(&bytes, max_bytes, full);

    let out = ChunkDumpOutput {
        path: entry_path.to_string(),
        chunk_index,
        decoded,
        chunk_info: chunk,
        byte_len: bytes.len(),
        truncated,
        preview_hex: hex_encode(preview),
    };

    match format {
        InspectOutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&out)?);
        }
        InspectOutputFormat::Text => {
            println!("entry      : {}", out.path);
            println!("chunk      : {}", out.chunk_index);
            println!("decoded    : {}", out.decoded);
            println!(
                "meta       : off={} stored={} raw={} codec={:?} crc={:08x}",
                out.chunk_info.data_offset,
                format_bytes(out.chunk_info.compressed_size as u64),
                format_bytes(out.chunk_info.uncompressed_size as u64),
                out.chunk_info.codec,
                out.chunk_info.checksum
            );
            println!("size       : {}", format_bytes(out.byte_len as u64));
            println!("truncated  : {}", out.truncated);
            println!("hex        : {}", out.preview_hex);
        }
    }

    Ok(())
}

fn load_v2_package(path: &Path) -> Result<LiveShadePackage> {
    LiveShadePackage::load(path).map_err(|err| {
        let msg = err.to_string();
        if msg.contains("bad magic") {
            anyhow::anyhow!("Unsupported .shade package format")
        } else {
            anyhow::anyhow!(msg)
        }
    })
}

fn print_verify_report_text(report: &VerifyReport) {
    println!("Verify mode   : {:?}", report.mode);
    println!("entries       : {}", report.total_entries);
    println!("chunks total  : {}", report.total_chunks);
    println!("chunks checked: {}", report.checked_chunks);
    println!(
        "result        : {}",
        if report.ok { "ok" } else { "failed" }
    );

    if !report.failures.is_empty() {
        println!("failures:");
        for failure in &report.failures {
            match failure.chunk_index {
                Some(chunk_idx) => {
                    println!(
                        "  {} chunk={} :: {}",
                        failure.path, chunk_idx, failure.message
                    );
                }
                None => {
                    println!("  {} :: {}", failure.path, failure.message);
                }
            }
        }
    }
}

fn select_preview(bytes: &[u8], max_bytes: usize, full: bool) -> (&[u8], bool) {
    if full {
        (bytes, false)
    } else {
        let end = std::cmp::min(max_bytes, bytes.len());
        (&bytes[0..end], end < bytes.len())
    }
}

fn safe_ratio_u32(num: u32, den: u32) -> f64 {
    if den == 0 {
        1.0
    } else {
        num as f64 / den as f64
    }
}

fn truncate_for_table(input: &str, max_len: usize) -> String {
    if input.len() <= max_len {
        return input.to_string();
    }
    if max_len <= 1 {
        return "…".to_string();
    }
    let keep = max_len.saturating_sub(1);
    format!("{}…", &input[..keep])
}

fn hex_preview(bytes: &[u8], max_bytes: usize) -> String {
    let end = std::cmp::min(max_bytes, bytes.len());
    let mut hex = hex_encode(&bytes[0..end]);
    if end < bytes.len() {
        hex.push('…');
    }
    hex
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{:02x}", b);
    }
    out
}

fn format_signed_bytes(value: i64) -> String {
    if value < 0 {
        format!("-{}", format_bytes(value.unsigned_abs()))
    } else {
        format_bytes(value as u64)
    }
}

fn format_bytes(value: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;

    let v = value as f64;
    if v >= GB {
        format!("{:.2}GiB", v / GB)
    } else if v >= MB {
        format!("{:.2}MiB", v / MB)
    } else if v >= KB {
        format!("{:.2}KiB", v / KB)
    } else {
        format!("{}B", value)
    }
}

#[cfg(test)]
mod tests {
    use super::{collect_transition_outputs, collect_transition_usage_output};
    use kroma_shared::types::ShadeConfig;

    #[test]
    fn collects_transition_usage_and_defs() {
        let config: ShadeConfig = toml::from_str(
            r#"
[meta]
name = "Inspect Transition Test"
author = "Kroma"

[states.active]
length = 0.0

[transitions.fade]
shader = "assets/fade.frag"
duration = 0.6

[transitions_usage]
on_load_to_active = "kroma.fade"
on_active_to_unload = "incoming.fade:0.2"
"#,
        )
        .expect("valid config");

        let usage = collect_transition_usage_output(&config.transitions_usage);
        assert_eq!(usage.on_load_to_active.as_deref(), Some("kroma.fade"));
        assert_eq!(
            usage.on_active_to_unload.as_deref(),
            Some("incoming.fade:0.2")
        );

        let transitions = collect_transition_outputs(&config);
        assert_eq!(transitions.len(), 1);
        assert_eq!(transitions[0].id, "fade");
        assert_eq!(transitions[0].shader, "assets/fade.frag");
        assert!((transitions[0].duration - 0.6).abs() < 1e-6);
    }
}
