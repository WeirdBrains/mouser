//! llama.cpp inside Mouser (P4b). Part of every build through mouser-app's
//! default `local-llm` feature; [`register`] plugs it into
//! `mouser_core::models::local`. llama.cpp starts lazily on the first local
//! chat, and without a usable GPU (no Vulkan driver) it runs on the CPU.
//! Prompts use the model's own chat template (rendered by core) and tool calls
//! the `<tool_call>` tag format parsed by `models::local::ToolParser`.

use anyhow::{Context, Result, anyhow, bail};
use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::LlamaModel;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::sampling::LlamaSampler;
use mouser_core::models::local::{self, LocalBackend, ToolParser};
use mouser_core::models::{ChatRequest, ChatSink, Event};
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

pub const DEFAULT_CONTEXT: u32 = 8192;
pub const DEFAULT_MAX_TOKENS: u32 = 1024;
const BATCH: usize = 512;

fn backend() -> Result<&'static LlamaBackend> {
    static B: OnceLock<Result<LlamaBackend, String>> = OnceLock::new();
    B.get_or_init(|| {
        LlamaBackend::init()
            .map(|mut b| {
                b.void_logs();
                b
            })
            .map_err(|e| e.to_string())
    })
    .as_ref()
    .map_err(|e| anyhow!("llama.cpp: {e}"))
}

struct Llama {
    /// The loaded model (kept until another file is asked for).
    model: Mutex<Option<(PathBuf, LlamaModel)>>,
}

/// Register the built-in llama.cpp backend (call once at startup).
pub fn register() {
    local::register(Box::new(Llama { model: Mutex::new(None) }));
}

impl LocalBackend for Llama {
    fn describe(&self) -> String {
        let gpu = backend().map(|b| b.supports_gpu_offload()).unwrap_or(false);
        format!("llama.cpp ({})", if gpu { if cfg!(target_os = "macos") { "Metal" } else { "Vulkan" } } else { "CPU" })
    }

    fn chat(&self, path: &Path, req: &ChatRequest, sink: &mut dyn ChatSink) -> Result<()> {
        let b = backend()?;
        let prefs = local::prefs();
        let mut guard = self.model.lock().map_err(|_| anyhow!("model lock poisoned"))?;
        let t0 = std::time::Instant::now();
        if guard.as_ref().is_none_or(|(p, _)| p != path) {
            *guard = None;
            let params = LlamaModelParams::default().with_n_gpu_layers(prefs.gpu_layers.unwrap_or(999));
            let m = LlamaModel::load_from_file(b, path, &params).with_context(|| format!("loading {}", path.display()))?;
            *guard = Some((path.to_path_buf(), m));
        }
        let (_, model) = guard.as_ref().context("model")?;
        let load = t0.elapsed();

        let turns = local::turns(req);
        let prompt = match model.meta_val_str("tokenizer.chat_template") {
            Ok(t) => local::render(&t, &turns, "").unwrap_or_else(|_| local::render_fallback(&turns)),
            Err(_) => local::render_fallback(&turns),
        };
        let vocab = model.vocab();
        let mut tokens = vocab.tokenize(prompt.as_bytes(), true, true);
        if tokens.is_empty() {
            bail!("empty prompt");
        }
        let n_ctx = prefs.context.unwrap_or(DEFAULT_CONTEXT);
        let max_new = req.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS) as usize;
        if tokens.len() + 8 >= n_ctx as usize {
            // Keep the end of an over-long prompt.
            tokens.drain(..tokens.len() + 8 + max_new.min(n_ctx as usize / 4) - n_ctx as usize);
        }
        let cparams = LlamaContextParams::default().with_n_ctx(NonZeroU32::new(n_ctx)).with_n_batch(BATCH as u32);
        let mut ctx = model.new_context(b, cparams).context("llama.cpp context")?;

        let t_prompt = std::time::Instant::now();
        // Prompt, in batches; logits only for the last token.
        let mut batch = LlamaBatch::new(BATCH, 1);
        let mut pos = 0i32;
        for chunk in tokens.chunks(BATCH) {
            batch.clear();
            for (i, t) in chunk.iter().enumerate() {
                let last = pos as usize + i + 1 == tokens.len();
                batch.add(*t, pos + i as i32, &[0], last)?;
            }
            ctx.decode(&mut batch).context("llama.cpp decode")?;
            pos += chunk.len() as i32;
        }

        let prompt_time = t_prompt.elapsed();
        let t_gen = std::time::Instant::now();
        let temp = req.temperature.unwrap_or(1.0);
        let mut sampler = if temp <= 0.0 {
            LlamaSampler::greedy()
        } else {
            LlamaSampler::chain_simple([
                LlamaSampler::top_k(64),
                LlamaSampler::top_p(0.95, 1),
                LlamaSampler::temp(temp),
                LlamaSampler::dist(0x6d6f7573),
            ])
        };
        let mut parser = ToolParser::default();
        let mut pending: Vec<u8> = Vec::new();
        let mut produced = 0usize;
        let mut stop = "end";
        let mut idx = batch.n_tokens() - 1;
        loop {
            if sink.cancelled() {
                return Ok(());
            }
            if produced >= max_new {
                stop = "max_tokens";
                break;
            }
            let tok = sampler.sample(&ctx, idx);
            sampler.accept(tok);
            if vocab.is_eog(tok) {
                break;
            }
            pending.extend(vocab.token_to_piece(tok, false, None));
            // Emit only complete UTF-8.
            let valid = match std::str::from_utf8(&pending) {
                Ok(_) => pending.len(),
                Err(e) => e.valid_up_to(),
            };
            if valid > 0 {
                let s = String::from_utf8_lossy(&pending[..valid]).into_owned();
                pending.drain(..valid);
                parser.feed(&s, sink);
            }
            produced += 1;
            batch.clear();
            batch.add(tok, pos, &[0], true)?;
            pos += 1;
            idx = 0;
            if pos as u32 >= n_ctx {
                stop = "max_tokens";
                break;
            }
            ctx.decode(&mut batch).context("llama.cpp decode")?;
        }
        if !pending.is_empty() {
            parser.feed(&String::from_utf8_lossy(&pending), sink);
        }
        parser.finish(sink);
        if std::env::var_os("MOUSER_LLM_TIMINGS").is_some() {
            let (p, g) = (prompt_time.as_secs_f64(), t_gen.elapsed().as_secs_f64());
            eprintln!(
                "\n[llama.cpp] load {:.2}s; prompt {} tokens in {:.0} ms ({:.0} t/s); generated {produced} tokens in {:.0} ms ({:.1} t/s)",
                load.as_secs_f64(),
                tokens.len(),
                p * 1e3,
                tokens.len() as f64 / p.max(1e-6),
                g * 1e3,
                produced as f64 / g.max(1e-6)
            );
        }
        sink.event(Event::Usage { input: tokens.len() as u64, output: produced as u64 });
        sink.event(Event::Done { stop: if parser.calls > 0 { "tool_use".into() } else { stop.into() } });
        Ok(())
    }
}
