use anyhow::{bail, Context, Result};
use clap::{CommandFactory, Parser};
use std::fs;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use beheader::{append_zip_to_output, build_polyglot, convert_image_to_png, PolyglotConfig};

const DEFAULT_TEMP_DIR: &str = ".";

const NOTICE_HELP_MSG: &str = "\
Notice: 
    Video conversion produces temporary files in a temp directory.
    Use --temp-dir to specify a custom location.\
";

#[derive(Parser, Debug)]
#[command(
    name = "beheader",
    about = "Polyglot generator for media files",
    after_help = NOTICE_HELP_MSG,
)]
struct Args {
    /// Path of resulting polyglot file
    output: PathBuf,

    /// Path of input image file
    image: PathBuf,

    /// Path of input video (or audio) file, will be encoded by ffmpeg (produces temporary files)
    video: Option<PathBuf>,

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

    /// Path of a pre-encoded MP4 file (H.264 video or AAC audio), skips ffmpeg encoding
    #[arg(long = "preprocessed-mp4", conflicts_with = "video")]
    preprocessed_mp4: Option<PathBuf>,
}

fn parse_args_from_line(line: &str) -> Result<Args> {
    let tokens = shell_words::split(line).context("Failed to parse input line")?;
    let mut full_args = vec![std::env::args_os().next().unwrap_or_default()];
    for t in tokens {
        full_args.push(t.into());
    }
    Args::try_parse_from(full_args).context("Failed to parse arguments")
}

fn find_ffmpeg() -> Result<PathBuf> {
    let exe_name = if cfg!(windows) {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    };
    let candidates = vec![Path::new(exe_name).to_path_buf()];
    for c in &candidates {
        if c.exists() {
            return Ok(c.canonicalize()?);
        }
    }
    let download_links = if cfg!(windows) {
        "https://www.gyan.dev/ffmpeg/builds/ or https://github.com/BtbN/FFmpeg-Builds/releases"
    } else if cfg!(target_os = "macos") {
        "https://evermeet.cx/ffmpeg/ or via Homebrew: brew install ffmpeg"
    } else {
        "https://johnvansickle.com/ffmpeg/ or via your package manager (apt install ffmpeg, etc.)"
    };
    bail!(
        "ffmpeg not found. Please place {} in the current directory \
         or install it to your PATH.\n\
         Download ffmpeg: {}",
        exe_name,
        download_links
    );
}

fn run_ffmpeg(ffmpeg: &Path, args: &[&str], output: &Path) -> Result<()> {
    let output = Command::new(ffmpeg)
        .args(args)
        .arg(output)
        .output()
        .context("Failed to run ffmpeg")?;
    if !output.status.success() {
        eprintln!("ffmpeg stderr: {}", String::from_utf8_lossy(&output.stderr));
        bail!("ffmpeg failed to encode");
    }
    Ok(())
}

fn has_video_stream(ffmpeg: &Path, video_path: &Path) -> Result<bool> {
    let output = Command::new(ffmpeg)
        .arg("-i")
        .arg(video_path)
        .output()
        .context("Failed to run ffmpeg")?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    Ok(stderr.contains("Video:"))
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
    let tmp_mp4_1 = tmp_dir.join("_beheader_1.mp4");

    let _cleanup = scopeguard::guard((), |_| {
        let _ = fs::remove_file(&tmp_mp4_0);
        let _ = fs::remove_file(&tmp_mp4_1);
    });

    let png_data = convert_image_to_png(&args.image)?;

    if let Some(preprocessed) = &args.preprocessed_mp4 {
        fs::copy(preprocessed, &tmp_mp4_0).context("Failed to copy preprocessed MP4")?;
    } else if let Some(video) = &args.video {
        let ffmpeg = find_ffmpeg()?;
        let is_video = has_video_stream(&ffmpeg, video)?;
        if is_video {
            run_ffmpeg(
                &ffmpeg,
                &[
                    "-i",
                    video.to_str().unwrap(),
                    "-c:v",
                    "libx264",
                    "-strict",
                    "-2",
                    "-preset",
                    "slow",
                    "-pix_fmt",
                    "yuv420p",
                    "-vf",
                    "scale=trunc(iw/2)*2:trunc(ih/2)*2",
                    "-f",
                    "mp4",
                    "-y",
                ],
                &tmp_mp4_0,
            )?;
        } else {
            run_ffmpeg(
                &ffmpeg,
                &[
                    "-i",
                    video.to_str().unwrap(),
                    "-c:a",
                    "aac",
                    "-b:a",
                    "192k",
                    "-y",
                ],
                &tmp_mp4_0,
            )?;
        }
    } else {
        let minimal_ftyp: &[u8] = &[
            0x00, 0x00, 0x00, 0x20, 0x66, 0x74, 0x79, 0x70, 0x69, 0x73, 0x6f, 0x6d, 0x00, 0x00,
            0x02, 0x00, 0x69, 0x73, 0x6f, 0x6d, 0x69, 0x73, 0x6f, 0x32, 0x61, 0x76, 0x63, 0x31,
            0x6d, 0x70, 0x34, 0x31,
        ];
        fs::write(&tmp_mp4_0, minimal_ftyp).context("Failed to write minimal MP4")?;
    }

    let mp4_data = fs::read(&tmp_mp4_0).context("Failed to read MP4")?;

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
