use tokend::config;
use tokend::server;
use tokend::tokenizer;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "tokend",
    version,
    about = "Multi-model tokenizer daemon for LLM inference infrastructure"
)]
struct Cli {
    /// Path to config file
    #[arg(short, long, default_value = "tokend.yaml")]
    config: String,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the tokenizer server
    Serve,
    /// Run throughput benchmark per loaded model
    Bench {
        /// Number of iterations per model
        #[arg(short = 'n', long, default_value_t = 1000)]
        iterations: u32,
        /// Input text for benchmarking
        #[arg(
            short,
            long,
            default_value = "The quick brown fox jumps over the lazy dog. This is a benchmark sentence for tokenizer throughput testing."
        )]
        text: String,
    },
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Serve => {
            if let Err(e) = serve(&cli.config) {
                eprintln!("fatal: {e:#}");
                std::process::exit(1);
            }
        }
        Commands::Bench { iterations, text } => {
            if let Err(e) = bench(&cli.config, iterations, &text) {
                eprintln!("fatal: {e:#}");
                std::process::exit(1);
            }
        }
    }
}

fn serve(config_path: &str) -> anyhow::Result<()> {
    let cfg = config::Config::load(config_path)?;

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(server::run(cfg))
}

fn bench(config_path: &str, iterations: u32, text: &str) -> anyhow::Result<()> {
    let cfg = config::Config::load(config_path)?;

    let registry = tokenizer::TokenizerRegistry::new(&cfg.cache_dir);
    registry.load_from_config(&cfg);

    let models = registry.list_models();
    if models.is_empty() {
        anyhow::bail!("no tokenizers loaded — check config and HF_TOKEN");
    }

    println!(
        "tokend bench — {} model(s), {} iterations each\n",
        models.len(),
        iterations
    );

    for model in &models {
        let start = std::time::Instant::now();
        let mut total_tokens: u64 = 0;

        for _ in 0..iterations {
            match registry.tokenize(model, &[text], true, false) {
                Ok(results) => {
                    total_tokens += results[0].token_count as u64;
                }
                Err(e) => {
                    eprintln!("  {model}: error — {e}");
                    break;
                }
            }
        }

        let elapsed = start.elapsed();
        let tokens_per_sec = total_tokens as f64 / elapsed.as_secs_f64();
        let us_per_call = elapsed.as_micros() as f64 / iterations as f64;

        println!("  {model}");
        println!("    {iterations} iterations in {elapsed:.2?}");
        println!("    {us_per_call:.1} us/call, {tokens_per_sec:.0} tokens/sec");
        println!(
            "    {total_tokens} total tokens ({} tokens/call)\n",
            total_tokens / iterations as u64
        );
    }

    Ok(())
}
