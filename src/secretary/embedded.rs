use anyhow::{Context, Result};
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::config::EmbeddedConfig;
use super::Secretary;

/// Embedded LLM backend using llama-cpp-2.
/// Loads a GGUF model in-process with Metal GPU acceleration.
/// Inference runs in spawn_blocking to not block the Axum event loop.
pub struct EmbeddedSecretary {
    // We hold the backend + model behind a Mutex because
    // llama-cpp-2 contexts are !Send. One inference at a time.
    inner: Arc<Mutex<EmbeddedInner>>,
    context_size: u32,
}

struct EmbeddedInner {
    backend: llama_cpp_2::llama_backend::LlamaBackend,
    model: llama_cpp_2::model::LlamaModel,
}

// Safety: We ensure single-threaded access via Mutex
unsafe impl Send for EmbeddedInner {}
unsafe impl Sync for EmbeddedInner {}

impl EmbeddedSecretary {
    pub fn new(config: &EmbeddedConfig) -> Result<Self> {
        tracing::info!(
            model = %config.model_path.display(),
            gpu_layers = config.gpu_layers,
            "Loading GGUF model (this may take a few seconds)..."
        );

        let backend = llama_cpp_2::llama_backend::LlamaBackend::init()
            .context("Failed to init llama backend")?;

        let model_params = llama_cpp_2::model::params::LlamaModelParams::default()
            .with_n_gpu_layers(config.gpu_layers);

        let model = llama_cpp_2::model::LlamaModel::load_from_file(
            &backend,
            &config.model_path,
            &model_params,
        )
        .context("Failed to load GGUF model")?;

        tracing::info!("Model loaded successfully");

        Ok(Self {
            inner: Arc::new(Mutex::new(EmbeddedInner { backend, model })),
            context_size: config.context_size,
        })
    }
}

#[async_trait]
impl Secretary for EmbeddedSecretary {
    async fn extract(&self, prompt: &str, json_schema: &str) -> Result<String> {
        let inner = self.inner.clone();
        let prompt = prompt.to_string();
        let schema = json_schema.to_string();
        let ctx_size = self.context_size;

        // Run inference in a blocking thread to not stall the async runtime
        tokio::task::spawn_blocking(move || {
            let inner = inner.blocking_lock();
            run_inference(&inner, &prompt, &schema, ctx_size)
        })
        .await
        .context("Inference task panicked")?
    }

    fn name(&self) -> &str {
        "embedded (llama-cpp-2)"
    }
}

/// Actual inference logic — runs on a blocking thread.
fn run_inference(
    inner: &EmbeddedInner,
    prompt: &str,
    json_schema: &str,
    context_size: u32,
) -> Result<String> {
    use llama_cpp_2::context::params::LlamaContextParams;
    use llama_cpp_2::llama_batch::LlamaBatch;
    use llama_cpp_2::token::data_array::LlamaTokenDataArray;

    tracing::debug!(prompt_bytes = prompt.len(), context_size, "run_inference: starting");

    // Wrap in ChatML format for Qwen 2.5 Instruct (and other instruction-tuned models).
    // Without the template the model treats the prompt as a conversation to continue
    // rather than an instruction to follow, producing garbage instead of JSON.
    let formatted = format!(
        "<|im_start|>system\nYou are an expert technical analyst. Output only valid JSON, no prose or explanation.<|im_end|>\n<|im_start|>user\n{}<|im_end|>\n<|im_start|>assistant\n",
        prompt
    );

    // Create a fresh context for this inference
    let ctx_params = LlamaContextParams::default()
        .with_n_ctx(std::num::NonZeroU32::new(context_size));

    let mut ctx = inner.model.new_context(&inner.backend, ctx_params)
        .context("Failed to create inference context")?;
    tracing::debug!("run_inference: context created");

    // Tokenize — AddBos::Never since ChatML tokens serve as BOS signals
    let tokens = inner.model.str_to_token(&formatted, llama_cpp_2::model::AddBos::Never)
        .context("Failed to tokenize prompt")?;
    tracing::debug!(token_count = tokens.len(), "run_inference: tokenized");

    if tokens.len() as u32 >= context_size {
        anyhow::bail!(
            "Prompt ({} tokens) exceeds context size ({})",
            tokens.len(),
            context_size
        );
    }

    // Feed prompt tokens
    let mut prompt_batch = LlamaBatch::new(tokens.len(), 1);
    let last_idx = tokens.len() - 1;
    for (i, &token) in tokens.iter().enumerate() {
        prompt_batch.add(token, i as i32, &[0], i == last_idx)
            .with_context(|| format!("Failed to add prompt token {} to batch", i))?;
    }
    ctx.decode(&mut prompt_batch).context("Failed to decode prompt")?;
    tracing::debug!("run_inference: prompt decoded, starting generation");

    // Generate output tokens — use a fresh single-token batch each step.
    // Re-using a cleared batch leaves n_tokens == 0 in llama-cpp-2's internals.
    let max_output_tokens = 2048; // Decisions JSON shouldn't exceed this
    let mut output_tokens = Vec::new();
    let eos = inner.model.token_eos();

    let _grammar_hint = json_schema; // Reserved for GBNF grammar integration

    for step in 0..max_output_tokens {
        let logits = ctx.candidates();
        let mut candidates = LlamaTokenDataArray::from_iter(logits, false);

        // Greedy sampling — always pick the highest-probability token.
        // Random-seeded sampling (sample_token(step)) hits EOS prematurely.
        let token = candidates.sample_token_greedy();

        if token == eos {
            tracing::debug!(step, "run_inference: hit EOS token");
            break;
        }

        // Position of this generated token in the full sequence
        let pos = (tokens.len() + output_tokens.len()) as i32;
        output_tokens.push(token);

        // Fresh batch — avoids the cleared-batch n_tokens == 0 bug
        let mut gen_batch = LlamaBatch::new(1, 1);
        gen_batch.add(token, pos, &[0], true)
            .with_context(|| format!("Failed to add generated token at step {}", step))?;
        ctx.decode(&mut gen_batch)
            .with_context(|| format!("Failed to decode at generation step {}", step))?;
    }

    tracing::debug!(generated = output_tokens.len(), "run_inference: generation complete");

    // Detokenize output tokens to text
    let mut output = String::new();
    for &token in &output_tokens {
        match inner.model.token_to_str(token, llama_cpp_2::model::Special::Tokenize) {
            Ok(piece) => output.push_str(&piece),
            Err(e) => tracing::warn!(token = token.0, error = %e, "Failed to detokenize token"),
        }
    }
    tracing::debug!(output_bytes = output.len(), output_preview = %output.chars().take(200).collect::<String>(), "run_inference: detokenized");

    Ok(output.trim().to_string())
}
