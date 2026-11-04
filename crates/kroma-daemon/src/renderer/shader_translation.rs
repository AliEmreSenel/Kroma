use anyhow::Result;
use std::{
    collections::VecDeque,
    sync::{LazyLock, Mutex},
};

const CACHE_BYTES: usize = 2 * 1024 * 1024;
const CACHE_ENTRIES: usize = 16;

#[derive(Default)]
struct ShaderCache {
    entries: VecDeque<(String, String)>,
    bytes: usize,
}

impl ShaderCache {
    fn get(&mut self, source: &str) -> Option<String> {
        let index = self.entries.iter().position(|(key, _)| key == source)?;
        let entry = self.entries.remove(index)?;
        let output = entry.1.clone();
        self.entries.push_back(entry);
        Some(output)
    }

    fn insert(&mut self, source: &str, output: &str) {
        let size = source.len() + output.len();
        if size > CACHE_BYTES {
            return;
        }
        if let Some(index) = self.entries.iter().position(|(key, _)| key == source) {
            let old = self.entries.remove(index).expect("entry exists");
            self.bytes -= old.0.len() + old.1.len();
        }
        while self.bytes + size > CACHE_BYTES || self.entries.len() >= CACHE_ENTRIES {
            let old = self
                .entries
                .pop_front()
                .expect("cache contains evictable entries");
            self.bytes -= old.0.len() + old.1.len();
        }
        self.entries.push_back((source.into(), output.into()));
        self.bytes += size;
    }
}

static SHADERS: LazyLock<Mutex<ShaderCache>> = LazyLock::new(|| Mutex::new(ShaderCache::default()));

/// Translates fragment GLSL into WGSL expected by the renderer pipeline.
///
/// The translator injects the custom uniform storage declaration when missing,
/// compiles GLSL with shaderc, then validates and writes WGSL via naga.
pub(super) fn glsl_to_wgsl(glsl_source: &str) -> Result<String> {
    if let Some(output) = SHADERS
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(glsl_source)
    {
        let _timing = crate::timing::scope("shader.cache_hit");
        return Ok(output);
    }
    // The complete source is the key, so hot reload cannot reuse stale code.
    // Do not hold the cache lock while invoking native compilers or cache failures.
    let output = compile_glsl(glsl_source)?;
    SHADERS
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .insert(glsl_source, &output);
    Ok(output)
}

fn compile_glsl(glsl_source: &str) -> Result<String> {
    let _timing = crate::timing::scope("shader.compile");
    use naga::back::wgsl;
    use naga::valid::{Capabilities, ValidationFlags, Validator};

    let glsl_source = if !glsl_source.contains("CustomUniforms") {
        if let Some(pos) = glsl_source.find("layout(location = 0) out vec4") {
            let (before, after) = glsl_source.split_at(pos);
            format!(
                "{}// Custom uniform storage buffer - access via custom_data[index]\nlayout(set = 0, binding = 1) readonly buffer CustomUniforms {{\n    float custom_data[32];\n}};\n\n{}",
                before, after
            )
        } else {
            let mut lines = glsl_source.lines();
            let first_line = lines.next().unwrap_or("");
            if first_line.starts_with("#version") {
                format!(
                    "{}\n\n// Custom uniform storage buffer - access via custom_data[index]\nlayout(set = 0, binding = 1) readonly buffer CustomUniforms {{\n    float custom_data[32];\n}};\n\n{}",
                    first_line,
                    lines.collect::<Vec<_>>().join("\n")
                )
            } else {
                format!(
                    "// Custom uniform storage buffer - access via custom_data[index]\nlayout(set = 0, binding = 1) readonly buffer CustomUniforms {{\n    float custom_data[32];\n}};\n\n{}",
                    glsl_source
                )
            }
        }
    } else {
        glsl_source.to_string()
    };

    let compiler = shaderc::Compiler::new()
        .map_err(|_| anyhow::anyhow!("Failed to create shaderc compiler"))?;
    let mut options = shaderc::CompileOptions::new()
        .map_err(|_| anyhow::anyhow!("Failed to create shaderc compile options"))?;
    options.set_target_env(
        shaderc::TargetEnv::Vulkan,
        shaderc::EnvVersion::Vulkan1_0 as u32,
    );
    options.set_source_language(shaderc::SourceLanguage::GLSL);
    options.set_target_spirv(shaderc::SpirvVersion::V1_0);
    options.set_auto_bind_uniforms(false);

    let binary = compiler
        .compile_into_spirv(
            &glsl_source,
            shaderc::ShaderKind::Fragment,
            "shader.frag",
            "main",
            Some(&options),
        )
        .map_err(|e| {
            log::error!("shaderc GLSL compile error:\n{}", e);
            anyhow::anyhow!("shaderc GLSL compile error: {}", e)
        })?;

    if binary.get_num_warnings() > 0 {
        log::warn!("shaderc warnings:\n{}", binary.get_warning_messages());
    }

    let spirv_bytes = binary.as_binary();

    let spv_options = naga::front::spv::Options {
        adjust_coordinate_space: false,
        strict_capabilities: false,
        block_ctx_dump_prefix: None,
    };

    let module = naga::front::spv::parse_u8_slice(bytemuck::cast_slice(spirv_bytes), &spv_options)
        .map_err(|e| {
            log::error!("SPIR-V parse error: {}", e);
            anyhow::anyhow!("SPIR-V parse error: {}", e)
        })?;

    let mut validator = Validator::new(ValidationFlags::all(), Capabilities::all());
    let info = validator.validate(&module).map_err(|e| {
        log::error!("Shader validation error: {}", e);
        anyhow::anyhow!("Shader validation error: {}", e)
    })?;

    let mut wgsl_source = wgsl::write_string(&module, &info, wgsl::WriterFlags::empty())
        .map_err(|e| anyhow::anyhow!("WGSL write error: {}", e))?;

    if let Some(pos) = wgsl_source.find("fn main(") {
        wgsl_source.replace_range(pos..pos + 8, "fn fs_main(");
    }

    Ok(wgsl_source)
}

/// Blocks on wgpu futures with a tiny single-threaded executor.
pub(super) fn pollster_block<F: std::future::Future>(f: F) -> F::Output {
    futures_lite_block_on(f)
}

/// Minimal spin-loop executor used for short native wgpu futures.
fn futures_lite_block_on<F: std::future::Future>(f: F) -> F::Output {
    use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

    fn raw_waker() -> RawWaker {
        fn no_op(_: *const ()) {}
        fn clone(_: *const ()) -> RawWaker {
            raw_waker()
        }
        let vtable = &RawWakerVTable::new(clone, no_op, no_op, no_op);
        RawWaker::new(std::ptr::null(), vtable)
    }

    let waker = unsafe { Waker::from_raw(raw_waker()) };
    let mut cx = Context::from_waker(&waker);
    let mut f = std::pin::pin!(f);

    loop {
        match f.as_mut().poll(&mut cx) {
            Poll::Ready(val) => return val,
            Poll::Pending => {
                std::hint::spin_loop();
            }
        }
    }
}

#[cfg(test)]
mod cache_tests {
    use super::*;

    #[test]
    fn cache_is_bounded_and_repeated_sources_replace_their_accounting() {
        let mut cache = ShaderCache::default();
        cache.insert("a", "first");
        cache.insert("a", "replacement");
        assert_eq!(cache.bytes, "areplacement".len());
        assert_eq!(cache.get("a").as_deref(), Some("replacement"));
        for i in 0..CACHE_ENTRIES + 2 {
            cache.insert(&format!("source-{i}"), "wgsl");
        }
        assert_eq!(cache.entries.len(), CACHE_ENTRIES);
        assert!(cache.get("source-0").is_none());
        cache.insert("large", &"x".repeat(CACHE_BYTES));
        assert!(cache.get("large").is_none());
        assert!(cache.bytes <= CACHE_BYTES);
    }

    #[test]
    fn hot_reload_changes_recompile_and_errors_do_not_poison_the_cache() {
        let source =
            "#version 450\nlayout(location=0) out vec4 color;\nvoid main(){color=vec4(0.25);}";
        let first = glsl_to_wgsl(source).unwrap();
        assert_eq!(first, glsl_to_wgsl(source).unwrap());
        assert_ne!(
            first,
            glsl_to_wgsl(&source.replace("0.25", "0.75")).unwrap()
        );
        assert!(glsl_to_wgsl("#version 450\ninvalid shader").is_err());
        assert_eq!(first, glsl_to_wgsl(source).unwrap());
    }
}
