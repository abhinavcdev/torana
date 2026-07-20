//! Sandboxed request-filter plugins compiled to WASM.
//!
//! This is intentionally a small ABI, not a general extension API: a plugin
//! sees only the request method and path, and returns either "allow" or
//! "deny with an HTTP status". No host functions are exposed, so a plugin
//! cannot touch the filesystem, network, or process — wasmtime's sandbox is
//! the entire security boundary. Execution is fuel-limited so a plugin
//! cannot hang a request by looping forever.
//!
//! A plugin module must export:
//! - `memory`: the standard WASM linear memory
//! - `alloc(len: i32) -> i32`: allocate `len` bytes, return a pointer
//! - `on_request(method_ptr, method_len, path_ptr, path_len) -> i32`:
//!   0 = allow; any value in 400..600 = deny with that status; anything
//!   else = deny with 403.

use anyhow::Context;
use std::collections::HashMap;
use std::sync::Arc;
use wasmtime::{Config as WasmConfig, Engine, Instance, Memory, Module, Store, TypedFunc};

const FUEL_PER_CALL: u64 = 5_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Allow,
    Deny(u16),
}

/// A compiled plugin module. Cheap to clone (shares the compiled `Module`
/// via wasmtime's internal `Arc`); each call gets a fresh `Store` so plugin
/// invocations never share state or interfere with each other.
#[derive(Clone)]
pub struct Plugin {
    engine: Engine,
    module: Module,
}

impl Plugin {
    pub fn load(path: &str) -> anyhow::Result<Self> {
        let mut config = WasmConfig::new();
        config.consume_fuel(true);
        let engine = Engine::new(&config).context("initializing wasmtime engine")?;

        let bytes = std::fs::read(path).with_context(|| format!("reading plugin '{}'", path))?;
        let module = Module::new(&engine, &bytes).with_context(|| {
            format!("compiling plugin '{}' (must be a valid WASM module)", path)
        })?;

        module
            .get_export("on_request")
            .context("plugin is missing the required export `on_request`")?;
        module
            .get_export("alloc")
            .context("plugin is missing the required export `alloc`")?;
        module
            .get_export("memory")
            .context("plugin is missing the exported `memory`")?;

        Ok(Plugin { engine, module })
    }

    /// Run the plugin against one request. Traps (including running out of
    /// fuel) are treated as a plugin bug, not a proxy bug, and denied with
    /// 500 rather than propagated as a proxy error.
    pub fn evaluate(&self, method: &str, path: &str) -> Verdict {
        match self.try_evaluate(method, path) {
            Ok(verdict) => verdict,
            Err(e) => {
                tracing::error!("plugin execution failed: {:#}", e);
                Verdict::Deny(500)
            }
        }
    }

    fn try_evaluate(&self, method: &str, path: &str) -> anyhow::Result<Verdict> {
        let mut store = Store::new(&self.engine, ());
        store
            .set_fuel(FUEL_PER_CALL)
            .context("setting fuel budget")?;

        let instance =
            Instance::new(&mut store, &self.module, &[]).context("instantiating plugin")?;
        let memory: Memory = instance
            .get_memory(&mut store, "memory")
            .context("plugin has no memory export")?;
        let alloc: TypedFunc<i32, i32> = instance
            .get_typed_func(&mut store, "alloc")
            .context("plugin `alloc` has the wrong signature (expected (i32) -> i32)")?;
        let on_request: TypedFunc<(i32, i32, i32, i32), i32> = instance
            .get_typed_func(&mut store, "on_request")
            .context("plugin `on_request` has the wrong signature")?;

        let method_ptr = write_bytes(&mut store, &memory, &alloc, method.as_bytes())?;
        let path_ptr = write_bytes(&mut store, &memory, &alloc, path.as_bytes())?;

        let code = on_request
            .call(
                &mut store,
                (method_ptr, method.len() as i32, path_ptr, path.len() as i32),
            )
            .context("plugin `on_request` trapped (possibly out of fuel)")?;

        Ok(match code {
            0 => Verdict::Allow,
            c if (400..600).contains(&c) => Verdict::Deny(c as u16),
            _ => Verdict::Deny(403),
        })
    }
}

fn write_bytes(
    store: &mut Store<()>,
    memory: &Memory,
    alloc: &TypedFunc<i32, i32>,
    bytes: &[u8],
) -> anyhow::Result<i32> {
    let ptr = alloc
        .call(&mut *store, bytes.len() as i32)
        .context("plugin alloc() call failed")?;
    memory
        .write(&mut *store, ptr as usize, bytes)
        .context("writing into plugin memory")?;
    Ok(ptr)
}

/// Compile every plugin referenced by the config, deduplicated by path.
/// Fails closed: a config that names a broken plugin fails validation/reload
/// rather than silently running unfiltered.
pub fn build_plugin_cache(
    config: &crate::config::Config,
) -> anyhow::Result<HashMap<String, Arc<Plugin>>> {
    let mut cache = HashMap::new();
    for route in &config.route {
        if let Some(path) = &route.plugin {
            if !cache.contains_key(path) {
                let plugin = Plugin::load(path).with_context(|| {
                    format!("route '{}': loading plugin '{}'", route.name, path)
                })?;
                cache.insert(path.clone(), Arc::new(plugin));
            }
        }
    }
    Ok(cache)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Compiles a tiny real WASM plugin with `rustc` and returns its path
    /// (kept alive via the returned TempDir). Skips the test (returns None)
    /// if the wasm32-unknown-unknown target isn't installed — CI installs
    /// it explicitly, but a stray dev machine shouldn't hard-fail on this.
    fn compile_test_plugin(source: &str) -> Option<(std::path::PathBuf, tempdir::TempDir)> {
        let dir = tempdir::TempDir::new();
        let src_path = dir.path().join("plugin.rs");
        let wasm_path = dir.path().join("plugin.wasm");
        std::fs::write(&src_path, source).unwrap();

        let status = std::process::Command::new("rustc")
            .args([
                "--target",
                "wasm32-unknown-unknown",
                "--crate-type",
                "cdylib",
                "-O",
                "-o",
            ])
            .arg(&wasm_path)
            .arg(&src_path)
            .status();

        match status {
            Ok(s) if s.success() && wasm_path.exists() => Some((wasm_path, dir)),
            _ => {
                eprintln!("skipping plugin test: rustc could not target wasm32-unknown-unknown");
                None
            }
        }
    }

    const ALLOW_ALL_SOURCE: &str = r#"
        #[no_mangle]
        pub extern "C" fn alloc(len: i32) -> i32 {
            let mut buf = Vec::<u8>::with_capacity(len as usize);
            let ptr = buf.as_mut_ptr();
            std::mem::forget(buf);
            ptr as i32
        }

        #[no_mangle]
        pub extern "C" fn on_request(_mp: i32, _ml: i32, _pp: i32, _pl: i32) -> i32 {
            0
        }
    "#;

    const DENY_POST_SOURCE: &str = r#"
        #[no_mangle]
        pub extern "C" fn alloc(len: i32) -> i32 {
            let mut buf = Vec::<u8>::with_capacity(len as usize);
            let ptr = buf.as_mut_ptr();
            std::mem::forget(buf);
            ptr as i32
        }

        #[no_mangle]
        pub extern "C" fn on_request(method_ptr: i32, method_len: i32, _pp: i32, _pl: i32) -> i32 {
            let method = unsafe {
                std::slice::from_raw_parts(method_ptr as *const u8, method_len as usize)
            };
            if method == b"POST" { 403 } else { 0 }
        }
    "#;

    const INFINITE_LOOP_SOURCE: &str = r#"
        #[no_mangle]
        pub extern "C" fn alloc(len: i32) -> i32 {
            let mut buf = Vec::<u8>::with_capacity(len as usize);
            let ptr = buf.as_mut_ptr();
            std::mem::forget(buf);
            ptr as i32
        }

        #[no_mangle]
        pub extern "C" fn on_request(_mp: i32, _ml: i32, _pp: i32, _pl: i32) -> i32 {
            loop {}
        }
    "#;

    #[test]
    fn allow_all_plugin_allows_everything() {
        let Some((wasm_path, _dir)) = compile_test_plugin(ALLOW_ALL_SOURCE) else {
            return;
        };
        let plugin = Plugin::load(wasm_path.to_str().unwrap()).expect("plugin should load");
        assert_eq!(plugin.evaluate("GET", "/"), Verdict::Allow);
        assert_eq!(plugin.evaluate("POST", "/anything"), Verdict::Allow);
    }

    #[test]
    fn plugin_can_deny_by_method() {
        let Some((wasm_path, _dir)) = compile_test_plugin(DENY_POST_SOURCE) else {
            return;
        };
        let plugin = Plugin::load(wasm_path.to_str().unwrap()).expect("plugin should load");
        assert_eq!(plugin.evaluate("GET", "/"), Verdict::Allow);
        assert_eq!(plugin.evaluate("POST", "/"), Verdict::Deny(403));
    }

    #[test]
    fn plugin_is_isolated_between_calls() {
        let Some((wasm_path, _dir)) = compile_test_plugin(DENY_POST_SOURCE) else {
            return;
        };
        let plugin = Plugin::load(wasm_path.to_str().unwrap()).expect("plugin should load");
        // Repeated calls on the same compiled Plugin must not leak state
        // (each call gets a fresh Store) and must not run out of fuel just
        // from being called many times.
        for _ in 0..50 {
            assert_eq!(plugin.evaluate("GET", "/"), Verdict::Allow);
        }
    }

    #[test]
    fn infinite_loop_is_fuel_limited_not_hung() {
        let Some((wasm_path, _dir)) = compile_test_plugin(INFINITE_LOOP_SOURCE) else {
            return;
        };
        let plugin = Plugin::load(wasm_path.to_str().unwrap()).expect("plugin should load");
        // Must return promptly (denied, since the trap is treated as a
        // plugin bug) rather than hang the calling thread.
        let start = std::time::Instant::now();
        assert_eq!(plugin.evaluate("GET", "/"), Verdict::Deny(500));
        assert!(
            start.elapsed() < std::time::Duration::from_secs(5),
            "fuel exhaustion should abort well under 5s"
        );
    }

    #[test]
    fn load_rejects_missing_exports() {
        // A module with no exports at all is missing `on_request`/`alloc`/
        // `memory` and must fail to load rather than fail confusingly later.
        let Some((wasm_path, _dir)) = compile_test_plugin("") else {
            return;
        };
        assert!(Plugin::load(wasm_path.to_str().unwrap()).is_err());
    }

    // Minimal TempDir so this test module doesn't need a dev-dependency.
    mod tempdir {
        pub struct TempDir(std::path::PathBuf);
        impl TempDir {
            pub fn new() -> Self {
                let dir = std::env::temp_dir().join(format!(
                    "torana-plugin-test-{}-{}",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_nanos()
                ));
                std::fs::create_dir_all(&dir).unwrap();
                TempDir(dir)
            }
            pub fn path(&self) -> &std::path::Path {
                &self.0
            }
        }
        impl Drop for TempDir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }
}
