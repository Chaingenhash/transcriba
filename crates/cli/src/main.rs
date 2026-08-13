use clap::Parser;
use std::path::PathBuf;
use transcriba_core::{decode, model_store, output, reflow, render, transcribe};

#[derive(Parser)]
#[command(
    name = "transcriba",
    about = "Transcribe an audio file to docx and pdf, fully offline"
)]
struct Args {
    /// Audio file to transcribe
    file: PathBuf,
    /// Language code, or "auto" to detect
    #[arg(long, default_value = "pt")]
    lang: String,
    /// Worker threads. Defaults to CPU count minus two.
    #[arg(long)]
    threads: Option<usize>,
}

fn main() {
    if let Err(e) = run() {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    eprintln!("Preparing the speech model...");
    let mut last = u64::MAX;
    let model_path = model_store::ensure_available(&mut |done, total| {
        let pct = total.map(|t| done * 100 / t.max(1)).unwrap_or(0);
        if pct != last {
            last = pct;
            eprint!("\r  {pct}%");
        }
    })?;
    if last != u64::MAX {
        eprintln!("\r  done.");
    }

    eprintln!("Decoding {}...", args.file.display());
    let audio = decode::decode(&args.file)?;
    eprintln!("  {:.1} minutes of audio", audio.duration / 60.0);

    let threads = args.threads.unwrap_or_else(transcribe::default_threads);
    eprintln!("Transcribing with {threads} threads...");
    let opts = transcribe::Options {
        language: args.lang.clone(),
        threads,
        model_path,
    };

    let mut last_pct = -1;
    let transcript = transcribe::transcribe(
        &audio,
        &opts,
        &mut |p| {
            if p != last_pct {
                last_pct = p;
                eprint!("\r  {p}%");
            }
        },
        &|| false,
    )?;
    eprintln!("\r  done — {} segments", transcript.cues.len());

    let blocks = reflow::reflow(&transcript.cues);
    let title = args
        .file
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let meta = render::DocumentMeta {
        title,
        duration: audio.duration,
        backend: transcript.backend.to_string(),
    };

    let docx_path = output::unique_path(&args.file.with_extension("docx"));
    std::fs::write(&docx_path, render::docx::render_docx(&blocks, &meta)?)?;

    let font_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/fonts");
    let pdf_path = output::unique_path(&args.file.with_extension("pdf"));
    std::fs::write(
        &pdf_path,
        render::pdf::render_pdf(&blocks, &meta, &font_dir)?,
    )?;

    eprintln!("Wrote {}", docx_path.display());
    eprintln!("Wrote {}", pdf_path.display());
    Ok(())
}
