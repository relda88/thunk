use std::num::NonZeroU32;
use std::path::Path;

use llama_cpp_2::{
    context::{
        params::{KvCacheType, LlamaContextParams},
        LlamaContext,
    },
    llama_backend::LlamaBackend,
    llama_batch::LlamaBatch,
    model::{params::LlamaModelParams, AddBos, LlamaModel},
    sampling::LlamaSampler,
    token::LlamaToken,
    TokenToStringError,
};

use crate::core::config::LlamaCppConfig;
use crate::core::error::{AppError, Result};
use crate::llm::backend::{BackendEvent, BackendStatus, BackendTimingStage};

pub(super) struct LoadedLlama {
    // ctx is declared first: Rust drops fields top-to-bottom, so ctx is released
    // before model. The 'static lifetime is manually upheld — the Box keeps the
    // model address stable across any moves of LoadedLlama.
    ctx: LlamaContext<'static>,
    pub(super) model: Box<LlamaModel>,
    pub(super) backend: LlamaBackend,
    pub(super) last_prefill_token_count: usize,
}

// SAFETY: LlamaContext wraps NonNull<llama_cpp_sys_2::llama_context> which is !Send.
// LoadedLlama has single-threaded exclusive ownership across all generate() calls.
unsafe impl Send for LoadedLlama {}

// RAII guard: redirects stderr (fd 2) to /dev/null on construction, restores on drop.
// Needed because native llama.cpp code (repack, sched_reserve, etc.) writes directly to
// stderr via fprintf, bypassing both llama_log_set and ggml_log_set callbacks entirely.
struct StderrSuppress {
    saved_fd: libc::c_int,
}

impl StderrSuppress {
    fn new() -> Self {
        let saved_fd = unsafe {
            let devnull = libc::open(
                b"/dev/null\0".as_ptr() as *const libc::c_char,
                libc::O_WRONLY,
            );
            let saved = libc::dup(2);
            if devnull >= 0 {
                libc::dup2(devnull, 2);
                libc::close(devnull);
            }
            saved
        };
        StderrSuppress { saved_fd }
    }
}

impl Drop for StderrSuppress {
    fn drop(&mut self) {
        if self.saved_fd >= 0 {
            unsafe {
                libc::dup2(self.saved_fd, 2);
                libc::close(self.saved_fd);
            }
        }
    }
}

pub(super) fn load_model(config: &LlamaCppConfig, model_path: &Path) -> Result<LoadedLlama> {
    if config.batch_tokens == 0 {
        return Err(AppError::Config(
            "llama.cpp requires `batch_tokens` to be greater than zero.".to_string(),
        ));
    }

    let mut backend = LlamaBackend::init().map_err(map_llama_error)?;
    if !config.show_native_logs {
        backend.void_logs();
        // void_logs() silences the llama_log_set callback; also silence ggml_log_set since
        // llama_log_set does not automatically cover GGML-level scheduler messages.
        unsafe extern "C" fn void_ggml_log(
            _level: llama_cpp_sys_2::ggml_log_level,
            _text: *const std::os::raw::c_char,
            _user_data: *mut std::os::raw::c_void,
        ) {
        }
        unsafe {
            llama_cpp_sys_2::ggml_log_set(Some(void_ggml_log), std::ptr::null_mut());
        }
    }

    let model_params = LlamaModelParams::default().with_n_gpu_layers(config.gpu_layers);
    let model = Box::new({
        // Native output (repack tensor messages, backend init prints) writes directly to
        // stderr via fprintf, bypassing log callbacks. Always suppress fd 2 here — the TUI
        // shares the terminal with stderr and must never receive raw native bytes regardless
        // of the show_native_logs setting.
        let _suppress = StderrSuppress::new();
        LlamaModel::load_from_file(&backend, model_path, &model_params).map_err(map_llama_error)?
    });

    // n_ubatch must be <= n_batch. Pin n_ubatch = n_batch to keep them consistent.
    let ctx_params = LlamaContextParams::default()
        .with_n_ctx(NonZeroU32::new(config.context_tokens))
        .with_n_batch(config.batch_tokens)
        .with_n_ubatch(config.batch_tokens)
        .with_type_k(KvCacheType::F16)
        .with_type_v(KvCacheType::F16)
        .with_offload_kqv(false);

    let ctx = {
        let _suppress = StderrSuppress::new();
        let raw_ctx = model.new_context(&backend, ctx_params).map_err(|error| {
            AppError::Runtime(format!(
                "{} (context_tokens={}, batch_tokens={}, n_ubatch={}, trained_context={})",
                error,
                config.context_tokens,
                config.batch_tokens,
                config.batch_tokens,
                model.n_ctx_train()
            ))
        })?;
        // SAFETY: model is heap-allocated (Box), so its address is stable across moves of
        // LoadedLlama. ctx is declared before model in the struct, ensuring it is dropped
        // first. The 'static lifetime is manually upheld by these two invariants.
        unsafe { std::mem::transmute::<LlamaContext<'_>, LlamaContext<'static>>(raw_ctx) }
    };

    Ok(LoadedLlama {
        ctx,
        model,
        backend,
        last_prefill_token_count: 0,
    })
}

/// GBNF grammar constraining llama.cpp output to valid thunk tool-call syntax.
/// Covers single-line bracket calls and zero-argument static calls only.
/// Block-form calls (edit_file, write_file) are excluded — constrained decoding
/// is disabled on MutationEnabled surface where block forms are expected.
pub(super) const TOOL_CALL_GRAMMAR: &str = r#"root        ::= pre-text tool-call
pre-text    ::= [^\[]*
tool-call   ::= named-call | static-call
named-call  ::= "[" tool-name ": " arg "]"
tool-name   ::= "read_file" | "list_dir" | "search_code" | "write_file" | "shell"
arg         ::= [^\]]+
static-call ::= "[" static-name "]"
static-name ::= "git_status" | "git_diff" | "git_diff_staged" | "git_log" | "git_branch"
"#;

/// GBNF grammar constraining llama.cpp output to an Aider-style SEARCH/REPLACE block.
/// Covers a single edit block per emission. Both search and replace sections allow
/// arbitrary line content. Active only on MutationEnabled surface.
pub(super) const EDIT_GRAMMAR: &str = r#"root           ::= pre-text edit-block
pre-text       ::= line*
edit-block     ::= "<<<<<<< SEARCH\n" section* "=======\n" section* ">>>>>>> REPLACE\n"
section        ::= line
line           ::= [^\n]* "\n"
"#;

pub(super) fn run_generation(
    loaded: &mut LoadedLlama,
    config: &LlamaCppConfig,
    prompt: &str,
    grammar: Option<&str>,
    on_event: &mut dyn FnMut(BackendEvent),
) -> Result<()> {
    use std::time::Instant;

    let context_tokens = config.context_tokens;
    let batch_tokens = config.batch_tokens;
    let max_tokens = config.max_tokens;
    let temperature = config.temperature;

    on_event(BackendEvent::StatusChanged(BackendStatus::Tokenizing));
    let t_tok_start = Instant::now();
    let tokens = loaded
        .model
        .str_to_token(prompt, AddBos::Always)
        .map_err(map_llama_error)?;
    on_event(BackendEvent::Timing {
        stage: BackendTimingStage::Tokenize,
        elapsed_ms: t_tok_start.elapsed().as_millis() as u64,
    });

    let context_limit = if context_tokens == 0 {
        loaded.model.n_ctx_train() as usize
    } else {
        context_tokens as usize
    };

    if tokens.len() >= context_limit {
        return Err(AppError::Runtime(format!(
            "Prompt exceeds llama.cpp context window ({} tokens >= {}). Try a shorter prompt or reduce injected context.",
            tokens.len(),
            context_limit
        )));
    }

    on_event(BackendEvent::Timing {
        stage: BackendTimingStage::PrefillStart,
        elapsed_ms: t_tok_start.elapsed().as_millis() as u64,
    });
    on_event(BackendEvent::StatusChanged(BackendStatus::Prefilling));
    let t_prefill_start = Instant::now();

    if tokens.len() < loaded.last_prefill_token_count {
        loaded
            .ctx
            .clear_kv_cache_seq(Some(0), Some(tokens.len() as u32), None)
            .ok();
        loaded.last_prefill_token_count = tokens.len();
    }
    let new_start = loaded.last_prefill_token_count;

    let mut batch = LlamaBatch::new(batch_tokens as usize, 1);
    let prefill_result = do_prefill(
        &mut loaded.ctx,
        &mut batch,
        &tokens,
        new_start,
        batch_tokens,
    );
    let prefill_result = match prefill_result {
        Err(_) if new_start > 0 => {
            loaded.ctx.clear_kv_cache();
            loaded.last_prefill_token_count = 0;
            do_prefill(&mut loaded.ctx, &mut batch, &tokens, 0, batch_tokens)
        }
        other => other,
    };
    prefill_result?;
    loaded.last_prefill_token_count = tokens.len();

    on_event(BackendEvent::Timing {
        stage: BackendTimingStage::PrefillDone,
        elapsed_ms: t_prefill_start.elapsed().as_millis() as u64,
    });

    let mut sampler_parts = vec![LlamaSampler::temp(temperature), LlamaSampler::dist(0)];
    if let Some(gbnf) = grammar {
        match LlamaSampler::grammar(&loaded.model, gbnf, "root") {
            Ok(g) => sampler_parts.insert(0, g),
            Err(e) => {
                eprintln!(
                    "[thunk] GBNF grammar compile failed, falling back to unconstrained: {e}"
                );
            }
        }
    }
    let mut sampler = LlamaSampler::chain_simple(sampler_parts);

    on_event(BackendEvent::StatusChanged(BackendStatus::Generating));
    let mut generated = 0usize;
    let mut current_pos = tokens.len() as i32;
    let t_gen_start = Instant::now();

    loop {
        let next_token = sampler.sample(&loaded.ctx, batch.n_tokens() - 1);

        if loaded.model.is_eog_token(next_token) {
            break;
        }

        let token_bytes = decode_token_bytes(&loaded.model, next_token).map_err(map_llama_error)?;

        on_event(BackendEvent::TextDelta(
            String::from_utf8_lossy(&token_bytes).to_string(),
        ));

        generated += 1;
        if generated >= max_tokens {
            break;
        }

        batch.clear();
        batch
            .add(next_token, current_pos, &[0], true)
            .map_err(map_llama_error)?;
        current_pos += 1;

        if current_pos as usize >= context_limit {
            break;
        }

        loaded.ctx.decode(&mut batch).map_err(map_llama_error)?;
    }

    loaded
        .ctx
        .clear_kv_cache_seq(Some(0), Some(tokens.len() as u32), Some(current_pos as u32))
        .ok();
    loaded.last_prefill_token_count = tokens.len();
    on_event(BackendEvent::Timing {
        stage: BackendTimingStage::GenerationDone,
        elapsed_ms: t_gen_start.elapsed().as_millis() as u64,
    });
    on_event(BackendEvent::TokenCounts {
        prompt: tokens.len() as u32,
        completion: generated as u32,
    });
    on_event(BackendEvent::Finished);
    Ok(())
}

fn do_prefill<'a>(
    ctx: &mut LlamaContext<'a>,
    batch: &mut LlamaBatch,
    tokens: &[LlamaToken],
    start: usize,
    batch_tokens: u32,
) -> Result<()> {
    let mut consumed = start;
    let last_prompt_idx = tokens.len() - 1;
    while consumed < tokens.len() {
        batch.clear();
        let end = (consumed + batch_tokens as usize).min(tokens.len());
        for (index, token) in tokens[consumed..end].iter().enumerate() {
            let position = (consumed + index) as i32;
            batch
                .add(*token, position, &[0], consumed + index == last_prompt_idx)
                .map_err(map_llama_error)?;
        }
        ctx.decode(batch).map_err(map_llama_error)?;
        consumed = end;
    }
    Ok(())
}

fn map_llama_error(error: impl ToString) -> AppError {
    AppError::Runtime(error.to_string())
}

fn decode_token_bytes(
    model: &LlamaModel,
    token: llama_cpp_2::token::LlamaToken,
) -> std::result::Result<Vec<u8>, TokenToStringError> {
    match model.token_to_piece_bytes(token, 8, false, None) {
        Err(TokenToStringError::InsufficientBufferSpace(size)) => model.token_to_piece_bytes(
            token,
            (-size)
                .try_into()
                .expect("token buffer size should be positive"),
            false,
            None,
        ),
        other => other,
    }
}
