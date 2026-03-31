use anyhow::{bail, Context, Result};
use clap::Parser;
use std::fs;
use std::io::{Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::process::Command;
use tempfile::TempDir;

#[derive(Parser, Debug)]
#[command(name = "beheader", about = "Polyglot generator for media files")]
struct Args {
    /// Path of resulting polyglot file
    output: PathBuf,

    /// Path of input image file
    image: PathBuf,

    /// Path of input video (or audio) file
    video: PathBuf,

    /// Path(s) of files to append without parsing
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
}

fn number_to_4b_le(num: u32) -> [u8; 4] {
    num.to_le_bytes()
}

fn number_to_4b_be(num: u32) -> [u8; 4] {
    num.to_be_bytes()
}

fn find_subarray(haystack: &[u8], needle: &[u8], start: usize) -> Option<usize> {
    if needle.is_empty() || start >= haystack.len() {
        return None;
    }
    haystack[start..]
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|i| start + i)
}

fn pad_left(s: &str, target_len: usize, pad_char: char) -> String {
    if s.len() >= target_len {
        s.to_string()
    } else {
        format!("{}{}", pad_char.to_string().repeat(target_len - s.len()), s)
    }
}

fn run_cmd(cmd: &mut Command, desc: &str) -> Result<std::process::Output> {
    let output = cmd
        .output()
        .with_context(|| format!("Failed to run {}", desc))?;
    if !output.status.success() {
        eprintln!("stderr: {}", String::from_utf8_lossy(&output.stderr));
        bail!("{} failed", desc);
    }
    Ok(output)
}

fn main() -> Result<()> {
    let args = Args::parse();

    let tmp_dir = TempDir::new()?;
    let tmp_path = tmp_dir.path();

    let tmp_png = tmp_path.join("input.png");
    let tmp_atom = tmp_path.join("atom.bin");
    let tmp_mp4_0 = tmp_path.join("0.mp4");
    let tmp_mp4_1 = tmp_path.join("1.mp4");
    let tmp_mp4_2 = tmp_path.join("2.mp4");
    let tmp_dir_zip = tmp_path.join("zipdir");
    let tmp_zip = tmp_path.join("merged.zip");

    // Convert input image to 32 bpp PNG, strip all metadata
    run_cmd(
        Command::new("convert")
            .arg(&args.image)
            .arg("-define")
            .arg("png:color-type=6")
            .arg("-depth")
            .arg("8")
            .arg("-alpha")
            .arg("on")
            .arg("-strip")
            .arg(&tmp_png),
        "convert (ImageMagick)",
    )?;

    let png_data = fs::read(&tmp_png).context("Failed to read converted PNG")?;

    // Probe video to determine if it has video stream
    let probe_output = run_cmd(
        Command::new("ffprobe")
            .arg("-v")
            .arg("error")
            .arg("-select_streams")
            .arg("v")
            .arg("-show_entries")
            .arg("stream=codec_type")
            .arg("-of")
            .arg("json")
            .arg(&args.video),
        "ffprobe",
    )?;

    let probe_json: serde_json::Value =
        serde_json::from_slice(&probe_output.stdout).context("Failed to parse ffprobe output")?;
    let is_video = probe_json["streams"]
        .as_array()
        .map_or(false, |arr| !arr.is_empty());

    // Re-encode input video to MP4 (or M4A for audio-only)
    if is_video {
        run_cmd(
            Command::new("ffmpeg")
                .arg("-i")
                .arg(&args.video)
                .arg("-c:v")
                .arg("libx264")
                .arg("-strict")
                .arg("-2")
                .arg("-preset")
                .arg("slow")
                .arg("-pix_fmt")
                .arg("yuv420p")
                .arg("-vf")
                .arg("scale=trunc(iw/2)*2:trunc(ih/2)*2")
                .arg("-f")
                .arg("mp4")
                .arg(&tmp_mp4_0),
            "ffmpeg (video)",
        )?;
    } else {
        run_cmd(
            Command::new("ffmpeg")
                .arg("-i")
                .arg(&args.video)
                .arg("-c:a")
                .arg("aac")
                .arg("-b:a")
                .arg("192k")
                .arg(&tmp_mp4_0),
            "ffmpeg (audio)",
        )?;
    }

    // Build ftyp buffer (256 + 32 bytes)
    let mut ftyp_buffer = vec![0u8; 288];

    // ICO signature (byte 2 = 1)
    ftyp_buffer[2] = 1;

    // Write "ftyp" atom name
    ftyp_buffer[4..8].copy_from_slice(b"ftyp");

    // Extended atom size marker
    ftyp_buffer[3] = 32;

    // Standard MP4 header data (second ftyp atom)
    ftyp_buffer[256..288].copy_from_slice(&[
        0x00, 0x00, 0x00, 0x20, 0x66, 0x74, 0x79, 0x70, // size=32, "ftyp"
        0x69, 0x73, 0x6f, 0x6d, // "isom"
        0x00, 0x00, 0x02, 0x00, // version
        0x69, 0x73, 0x6f, 0x6d, // "isom"
        0x69, 0x73, 0x6f, 0x32, // "iso2"
        0x61, 0x76, 0x63, 0x31, // "avc1"
        0x6d, 0x70, 0x34, 0x31, // "mp41"
    ]);

    // First image bit depth
    ftyp_buffer[12] = 32;

    // Image data size
    ftyp_buffer[14..18].copy_from_slice(&number_to_4b_le(png_data.len() as u32));

    // Write initial atom file for mp4edit
    fs::write(&tmp_atom, &ftyp_buffer)?;

    // Replace ftyp atom to measure offsets
    run_cmd(
        Command::new("mp4edit")
            .arg("--replace")
            .arg(format!("ftyp:{}", tmp_atom.display()))
            .arg(&tmp_mp4_0)
            .arg(&tmp_mp4_1),
        "mp4edit (replace ftyp)",
    )?;

    // Wrap HTML if provided
    let html_string = if let Some(html_path) = &args.html {
        let html_content = fs::read_to_string(html_path).context("Failed to read HTML file")?;
        format!(
            "--><style>body{{font-size:0}}</style><div style=font-size:initial>{}</div><!--",
            html_content
        )
    } else {
        String::new()
    };

    // Create skip atom buffer (contains PNG + HTML data)
    let html_bytes = html_string.as_bytes();
    let mut skip_buffer_data = Vec::with_capacity(png_data.len() + html_bytes.len());
    skip_buffer_data.extend_from_slice(html_bytes);
    skip_buffer_data.extend_from_slice(&png_data);

    let skip_buffer_head = {
        let mut head = [0u8; 8];
        let total_size = (skip_buffer_data.len() + 8) as u32;
        head[0..4].copy_from_slice(&number_to_4b_be(total_size));
        head[4..8].copy_from_slice(b"skip");
        head
    };

    let mut skip_buffer = Vec::with_capacity(skip_buffer_data.len() + 8);
    skip_buffer.extend_from_slice(&skip_buffer_head);
    skip_buffer.extend_from_slice(&skip_buffer_data);

    // Insert skip atom
    fs::write(&tmp_atom, &skip_buffer)?;
    run_cmd(
        Command::new("mp4edit")
            .arg("--insert")
            .arg(format!("skip:{}", tmp_atom.display()))
            .arg(&tmp_mp4_1)
            .arg(&tmp_mp4_2),
        "mp4edit (insert skip)",
    )?;

    // Find PNG offset in MP4 file
    let mp4_data = fs::read(&tmp_mp4_2).context("Failed to read MP4 file")?;
    let png_offset = find_subarray(&mp4_data, &skip_buffer_head, 0)
        .map(|pos| pos + 8 + html_bytes.len())
        .context("Failed to find PNG offset in MP4")?;

    // Set PNG data offset for first ICO image
    ftyp_buffer[18..22].copy_from_slice(&number_to_4b_le(png_offset as u32));

    // Set ICO image count to 1 and clear atom name
    ftyp_buffer[4..8].copy_from_slice(&[1, 0, 0, 0]);

    // Write supported brands
    ftyp_buffer[240..256].copy_from_slice(b"isomiso2avc1mp41");

    // Track free space start
    let mut atom_free_addr = 22;

    // Add extra data if provided
    let extra_data = if let Some(extra_path) = &args.extra {
        fs::read(extra_path).context("Failed to read extra file")?
    } else {
        Vec::new()
    };

    ftyp_buffer[atom_free_addr..atom_free_addr + extra_data.len()].copy_from_slice(&extra_data);
    atom_free_addr += extra_data.len();

    // Create HTML comment
    let comment = b"<!--";
    ftyp_buffer[atom_free_addr..atom_free_addr + 4].copy_from_slice(comment);
    atom_free_addr += 4;

    // Handle PDF if provided
    let pdf_data = if let Some(pdf_path) = &args.pdf {
        Some(fs::read(pdf_path).context("Failed to read PDF file")?)
    } else {
        None
    };

    let mp4_size = fs::metadata(&tmp_mp4_2)?.len() as usize;

    if let Some(ref pdf_buffer) = pdf_data {
        // First PDF pass - create early header and wrap MP4 in PDF object
        ftyp_buffer[atom_free_addr] = 0x0A;
        ftyp_buffer[atom_free_addr + 1..atom_free_addr + 10].copy_from_slice(&pdf_buffer[0..9]);
        atom_free_addr += 10;

        // Create PDF object string with dynamic length adjustment
        let mut offset = 30 + mp4_size.to_string().len();
        let obj_string;

        loop {
            offset -= 1;
            let length = mp4_size - atom_free_addr - extra_data.len() - offset;
            let candidate = format!("\n1 0 obj\n<</Length {}>>\nstream\n", length);
            if offset == candidate.len() {
                obj_string = candidate;
                break;
            }
        }

        let obj_bytes = obj_string.as_bytes();
        let end_addr = atom_free_addr + extra_data.len() + obj_bytes.len();
        if end_addr > ftyp_buffer.len() {
            bail!("PDF object string exceeds ftyp buffer size");
        }
        ftyp_buffer[atom_free_addr + extra_data.len()..end_addr].copy_from_slice(obj_bytes);
    }

    // Write final ftyp atom
    fs::write(&tmp_atom, &ftyp_buffer)?;

    // Replace ftyp and write output file
    run_cmd(
        Command::new("mp4edit")
            .arg("--replace")
            .arg(format!("ftyp:{}", tmp_atom.display()))
            .arg(&tmp_mp4_2)
            .arg(&args.output),
        "mp4edit (final replace)",
    )?;

    // Fix the bithack - split off extra ftyp atom
    {
        let mut file = fs::OpenOptions::new().write(true).open(&args.output)?;
        file.seek(SeekFrom::Start(3))?;
        file.write_all(&[0])?;
    }

    // Second PDF pass - close object and append PDF data
    if let Some(pdf_buffer) = pdf_data {
        let object_terminator = b"\nendstream\nendobj\n";
        let mut final_pdf = vec![0u8; pdf_buffer.len() + object_terminator.len() + 10];
        final_pdf[0..object_terminator.len()].copy_from_slice(object_terminator);
        final_pdf[object_terminator.len()..object_terminator.len() + pdf_buffer.len()]
            .copy_from_slice(&pdf_buffer);

        // Find cross-reference table
        let xref_start =
            find_subarray(&final_pdf, b"\nxref", 0).context("Failed to find xref table")? + 1;
        let offset_start = find_subarray(&final_pdf, b"\n0000000000", xref_start)
            .context("Failed to find offset in xref")?
            + 1;
        let startxref_start = find_subarray(&final_pdf, b"\nstartxref", xref_start)
            .context("Failed to find startxref")?
            + 1;
        let startxref_end = final_pdf[startxref_start + 11..]
            .iter()
            .position(|&b| b == 0x0A)
            .map(|p| startxref_start + 11 + p)
            .context("Failed to find end of startxref")?;

        let output_size = fs::metadata(&args.output)?.len() as usize;

        // Parse xref header to get entry count
        let xref_header = String::from_utf8_lossy(&final_pdf[xref_start..offset_start]);
        let count: usize = xref_header
            .trim()
            .split_whitespace()
            .last()
            .and_then(|s| s.parse().ok())
            .context("Failed to parse xref entry count")?;

        // Update all offsets
        let mut curr = offset_start;
        for _ in 0..count {
            let offset_str = String::from_utf8_lossy(&final_pdf[curr..curr + 10]);
            if let Ok(offset) = offset_str.trim().parse::<usize>() {
                let new_offset = offset + output_size + object_terminator.len();
                let padded = pad_left(&new_offset.to_string(), 10, '0');
                final_pdf[curr..curr + 10].copy_from_slice(padded.as_bytes());
            }
            curr = final_pdf[curr + 1..]
                .iter()
                .position(|&b| b == 0x0A)
                .map(|p| curr + 1 + p)
                .unwrap_or(curr + 1);
        }

        // Adjust startxref offset
        let startxref_str =
            String::from_utf8_lossy(&final_pdf[startxref_start + 10..startxref_end]);
        if let Ok(startxref) = startxref_str.trim().parse::<usize>() {
            let new_startxref = (startxref + output_size + object_terminator.len()).to_string();
            let new_bytes = new_startxref.as_bytes();
            final_pdf[startxref_start + 10..startxref_start + 10 + new_bytes.len()]
                .copy_from_slice(new_bytes);

            // Replace %%EOF just in case
            let eof_marker = b"\n%%EOF\n";
            let eof_pos = startxref_start + 10 + new_bytes.len();
            if eof_pos + eof_marker.len() <= final_pdf.len() {
                final_pdf[eof_pos..eof_pos + eof_marker.len()].copy_from_slice(eof_marker);
            }

            // Zero out remaining data
            let clear_start = startxref_start + new_bytes.len() + 17;
            for byte in final_pdf.iter_mut().skip(clear_start) {
                *byte = 0;
            }
        }

        // Append to output file
        let mut output_file = fs::OpenOptions::new()
            .write(true)
            .append(true)
            .open(&args.output)?;
        output_file.write_all(&final_pdf)?;
    }

    // Append any appendable files
    for path in &args.appendable {
        if !path.exists() {
            eprintln!("Warning: appendable file not found: {}", path.display());
            continue;
        }
        let mut output_file = fs::OpenOptions::new()
            .write(true)
            .append(true)
            .open(&args.output)?;
        let data = fs::read(path)?;
        output_file.write_all(&data)?;
    }

    // Handle ZIP archives
    if !args.zip.is_empty() {
        fs::create_dir_all(&tmp_dir_zip)?;

        // Extract all ZIP archives
        for zip_path in &args.zip {
            run_cmd(
                Command::new("unzip")
                    .arg("-d")
                    .arg(&tmp_dir_zip)
                    .arg(zip_path),
                &format!("unzip {}", zip_path.display()),
            )?;
        }

        // Create merged ZIP
        run_cmd(
            Command::new("zip")
                .arg("-r9")
                .arg(&tmp_zip)
                .arg(".")
                .current_dir(&tmp_dir_zip),
            "zip (merge)",
        )?;

        // Append ZIP to output
        {
            let mut output_file = fs::OpenOptions::new()
                .write(true)
                .append(true)
                .open(&args.output)?;
            let zip_data = fs::read(&tmp_zip)?;
            output_file.write_all(&zip_data)?;
        }

        // Fix self-extracting archive offset
        run_cmd(
            Command::new("zip").arg("-A").arg(&args.output),
            "zip -A (offset fix)",
        )?;
    }

    // TempDir will be automatically cleaned up when dropped

    println!("Polyglot file created: {}", args.output.display());
    Ok(())
}
