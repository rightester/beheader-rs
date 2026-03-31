use anyhow::{Context, Result};
use clap::{CommandFactory, Parser};
use std::fs;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

use beheader::{append_zip_to_output, build_polyglot, convert_image_to_png, PolyglotConfig};

const DEFAULT_TEMP_DIR: &str = ".";

#[derive(Parser, Debug)]
#[command(
    name = "beheader",
    about = "Polyglot generator for media files",
    after_help = "Notice: Video must be a pre-encoded MP4 (H.264/AAC). \
                  Use --temp-dir to specify a custom location for temporary files."
)]
struct Args {
    /// Path of resulting polyglot file
    output: PathBuf,

    /// Path of input image file
    image: PathBuf,

    /// Path of a pre-encoded MP4 file (H.264 video or AAC audio)
    mp4: Option<PathBuf>,

    /// Path(s) of files to append without parsing
    #[arg(trailing_var_arg = true)]
    appendable: Vec<PathBuf>,

    /// Path to HTML document
    #[arg(short = 'H', long = "html")]
    html: Option<PathBuf>,

    /// Path to PDF document
    #[arg(short = 'p', long = "pdf")]
    pdf: Option<PathBuf>,

    /// Path to ZIP-like archive (repeatable)
    #[arg(short = 'z', long = "zip")]
    zip: Vec<PathBuf>,

    /// Path to short (<200 byte) file to include near the header
    #[arg(short = 'e', long = "extra")]
    extra: Option<PathBuf>,

    /// Directory for temporary files
    #[arg(long = "temp-dir", value_name = "DIR")]
    temp_dir: Option<PathBuf>,
}

fn parse_args_from_line(line: &str) -> Result<Args> {
    let tokens = shell_words::split(line).context("Failed to parse input line")?;
    let mut full_args = vec![std::env::args_os().next().unwrap_or_default()];
    for t in tokens {
        full_args.push(t.into());
    }
    Args::try_parse_from(full_args).context("Failed to parse arguments")
}

fn main() -> Result<()> {
    let args = if std::env::args_os().len() <= 1 {
        let mut cmd = Args::command();
        cmd.print_long_help().ok();
        loop {
            eprintln!("\nEnter arguments or exit.");
            let mut input = String::new();
            io::stdin().lock().read_line(&mut input)?;
            let line = input.trim();
            if line.is_empty() {
                std::process::exit(0);
            }
            match parse_args_from_line(line) {
                Ok(a) => break a,
                Err(e) => {
                    eprintln!("{e}");
                    continue;
                }
            }
        }
    } else {
        Args::parse()
    };

    let tmp_dir = args
        .temp_dir
        .as_deref()
        .unwrap_or_else(|| Path::new(DEFAULT_TEMP_DIR));
    let tmp_mp4_0 = tmp_dir.join("_beheader_0.mp4");

    let _cleanup = scopeguard::guard((), |_| {
        let _ = fs::remove_file(&tmp_mp4_0);
    });

    let png_data = convert_image_to_png(&args.image)?;

    let mp4_data = if let Some(mp4_path) = &args.mp4 {
        fs::read(mp4_path).context("Failed to read MP4")?
    } else {
        let minimal_ftyp: &[u8] = &[
            0x00, 0x00, 0x00, 0x20, 0x66, 0x74, 0x79, 0x70, 0x69, 0x73, 0x6f, 0x6d, 0x00, 0x00,
            0x02, 0x00, 0x69, 0x73, 0x6f, 0x6d, 0x69, 0x73, 0x6f, 0x32, 0x61, 0x76, 0x63, 0x31,
            0x6d, 0x70, 0x34, 0x31,
        ];
        fs::write(&tmp_mp4_0, minimal_ftyp).context("Failed to write minimal MP4")?;
        fs::read(&tmp_mp4_0).context("Failed to read minimal MP4")?
    };

    let html_content = if let Some(html_path) = &args.html {
        Some(fs::read_to_string(html_path).context("Failed to read HTML")?)
    } else {
        None
    };

    let pdf_data = if let Some(pdf_path) = &args.pdf {
        Some(fs::read(pdf_path).context("Failed to read PDF")?)
    } else {
        None
    };

    let extra_data = if let Some(extra_path) = &args.extra {
        Some(fs::read(extra_path).context("Failed to read extra file")?)
    } else {
        None
    };

    let config = PolyglotConfig {
        png_data,
        mp4_data,
        html_content,
        pdf_data,
        extra_data,
    };

    let result = build_polyglot(&config)?;

    let mut out = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&args.output)?;
    out.write_all(&result.data)?;

    if let Some(pdf_suffix) = result.pdf_suffix {
        out.write_all(&pdf_suffix)?;
    }

    for path in &args.appendable {
        if !path.exists() {
            eprintln!("Warning: {} not found, skipping", path.display());
            continue;
        }
        let data = fs::read(path)?;
        out.write_all(&data)?;
    }

    if !args.zip.is_empty() {
        append_zip_to_output(&args.output, &args.zip)?;
    }

    println!("Polyglot created: {}", args.output.display());
    Ok(())
}
