use anyhow::{bail, Context, Result};
use byteorder::{BigEndian, ReadBytesExt, WriteBytesExt};
use clap::Parser;
use image::codecs::png::{CompressionType, PngEncoder};
use image::{ExtendedColorType, ImageEncoder};
use std::fs;
use std::io::{Cursor, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

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

fn find_subarray(haystack: &[u8], needle: &[u8], start: usize) -> Option<usize> {
    if needle.is_empty() || start >= haystack.len() {
        return None;
    }
    haystack[start..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|i| start + i)
}

fn pad_left(s: &str, target_len: usize, pad_char: char) -> String {
    if s.len() >= target_len {
        s.to_string()
    } else {
        format!("{}{}", pad_char.to_string().repeat(target_len - s.len()), s)
    }
}

fn read_box_header(data: &[u8], offset: usize) -> Option<(u64, [u8; 4])> {
    if offset + 8 > data.len() {
        return None;
    }
    let size = Cursor::new(&data[offset..offset + 4])
        .read_u32::<BigEndian>()
        .ok()? as u64;
    let mut box_type = [0u8; 4];
    box_type.copy_from_slice(&data[offset + 4..offset + 8]);
    Some((size, box_type))
}

fn find_boxes_recursive(
    data: &[u8],
    pos: usize,
    end: usize,
    target: &[u8; 4],
    results: &mut Vec<(u64, usize)>,
) {
    let mut p = pos;
    while p + 8 <= end {
        if let Some((size, box_type)) = read_box_header(data, p) {
            if size < 8 || p + size as usize > end {
                break;
            }
            if box_type == *target {
                results.push((size, p));
            } else if box_type != *b"mdat" && box_type != *b"skip" && size > 8 {
                // Recurse into container boxes (skip mdat and skip which contain raw data)
                find_boxes_recursive(data, p + 8, p + size as usize, target, results);
            }
            p += size as usize;
        } else {
            break;
        }
    }
}

fn find_all_boxes(data: &[u8], target: &[u8; 4]) -> Vec<(u64, usize)> {
    let mut results = Vec::new();
    find_boxes_recursive(data, 0, data.len(), target, &mut results);
    results
}

fn update_stco_offsets(mp4: &mut [u8], delta: u64) {
    let stco_boxes = find_all_boxes(mp4, b"stco");
    for (_size, pos) in stco_boxes {
        let pos = pos as usize;
        if pos + 16 > mp4.len() {
            continue;
        }
        let entry_count = Cursor::new(&mp4[pos + 12..pos + 16])
            .read_u32::<BigEndian>()
            .unwrap_or(0) as usize;
        let mut curr = pos + 16;
        for _ in 0..entry_count {
            if curr + 4 > mp4.len() {
                break;
            }
            let offset = Cursor::new(&mp4[curr..curr + 4])
                .read_u32::<BigEndian>()
                .unwrap_or(0) as u64;
            let new_offset = offset + delta;
            (&mut mp4[curr..curr + 4])
                .write_u32::<BigEndian>(new_offset as u32)
                .ok();
            curr += 4;
        }
    }
    let co64_boxes = find_all_boxes(mp4, b"co64");
    for (_size, pos) in co64_boxes {
        let pos = pos as usize;
        if pos + 16 > mp4.len() {
            continue;
        }
        let entry_count = Cursor::new(&mp4[pos + 12..pos + 16])
            .read_u32::<BigEndian>()
            .unwrap_or(0) as usize;
        let mut curr = pos + 16;
        for _ in 0..entry_count {
            if curr + 8 > mp4.len() {
                break;
            }
            let offset = Cursor::new(&mp4[curr..curr + 8])
                .read_u64::<BigEndian>()
                .unwrap_or(0);
            let new_offset = offset + delta;
            (&mut mp4[curr..curr + 8])
                .write_u64::<BigEndian>(new_offset)
                .ok();
            curr += 8;
        }
    }
}

fn replace_ftyp_box(mp4: &[u8], new_ftyp: &[u8]) -> Result<Vec<u8>> {
    let (ftyp_size, _) =
        read_box_header(mp4, 0).with_context(|| "Failed to read ftyp box header")?;
    let ftyp_end = ftyp_size as usize;
    let mut result = Vec::with_capacity(mp4.len() - ftyp_end + new_ftyp.len());
    result.extend_from_slice(new_ftyp);
    result.extend_from_slice(&mp4[ftyp_end..]);
    Ok(result)
}

fn insert_box_after_ftyp(mp4: &[u8], new_box: &[u8]) -> Result<Vec<u8>> {
    let (ftyp_size, _) =
        read_box_header(mp4, 0).with_context(|| "Failed to read ftyp box header")?;
    let ftyp_end = ftyp_size as usize;
    let mut result = Vec::with_capacity(mp4.len() + new_box.len());
    result.extend_from_slice(&mp4[..ftyp_end]);
    result.extend_from_slice(new_box);
    result.extend_from_slice(&mp4[ftyp_end..]);
    Ok(result)
}

fn run_ffmpeg(args: &[&str], tmp_mp4: &Path) -> Result<()> {
    let exe = Path::new("deps/ffmpeg.exe");
    if !exe.exists() {
        bail!("ffmpeg.exe not found at deps/ffmpeg.exe");
    }
    let output = Command::new(exe)
        .args(args)
        .arg(tmp_mp4)
        .output()
        .context("Failed to run ffmpeg")?;
    if !output.status.success() {
        eprintln!("ffmpeg stderr: {}", String::from_utf8_lossy(&output.stderr));
        bail!("ffmpeg failed");
    }
    Ok(())
}

fn has_video_stream(video_path: &Path) -> Result<bool> {
    let exe = Path::new("deps/ffmpeg.exe");
    if !exe.exists() {
        bail!("ffmpeg.exe not found at deps/ffmpeg.exe");
    }
    let output = Command::new(exe)
        .arg("-i")
        .arg(video_path)
        .output()
        .context("Failed to run ffmpeg")?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    Ok(stderr.contains("Video:"))
}

fn convert_image_to_png(image_path: &Path) -> Result<Vec<u8>> {
    let img = image::open(image_path).context("Failed to open input image")?;
    let rgba = img.into_rgba8();
    let mut buf = Vec::new();
    let encoder = PngEncoder::new_with_quality(
        &mut buf,
        CompressionType::Fast,
        image::codecs::png::FilterType::NoFilter,
    );
    encoder
        .write_image(
            rgba.as_raw(),
            rgba.width(),
            rgba.height(),
            ExtendedColorType::Rgba8,
        )
        .context("Failed to encode PNG")?;
    Ok(buf)
}

fn main() -> Result<()> {
    let args = Args::parse();

    let tmp_dir = args.output.parent().unwrap_or(Path::new("."));
    let tmp_mp4_0 = tmp_dir.join("_beheader_0.mp4");
    let tmp_mp4_1 = tmp_dir.join("_beheader_1.mp4");

    let _cleanup = scopeguard::guard((), |_| {
        let _ = fs::remove_file(&tmp_mp4_0);
        let _ = fs::remove_file(&tmp_mp4_1);
    });

    // Convert image to PNG using pure Rust
    let png_data = convert_image_to_png(&args.image)?;

    // Detect video stream and encode
    let is_video = has_video_stream(&args.video)?;
    if is_video {
        run_ffmpeg(
            &[
                "-i",
                args.video.to_str().unwrap(),
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
            &[
                "-i",
                args.video.to_str().unwrap(),
                "-c:a",
                "aac",
                "-b:a",
                "192k",
                "-y",
            ],
            &tmp_mp4_0,
        )?;
    }

    // Build ftyp buffer (288 bytes: 256 ICO header + 32 ftyp box)
    let mut ftyp_buffer = vec![0u8; 288];
    ftyp_buffer[2] = 1; // ICO reserved byte
    ftyp_buffer[4..8].copy_from_slice(b"ftyp");
    ftyp_buffer[3] = 32; // Extended size marker
    ftyp_buffer[12] = 32; // Bit depth
    ftyp_buffer[14..18].copy_from_slice(&(png_data.len() as u32).to_le_bytes());
    ftyp_buffer[256..288].copy_from_slice(&[
        0x00, 0x00, 0x00, 0x20, 0x66, 0x74, 0x79, 0x70, 0x69, 0x73, 0x6f, 0x6d, 0x00, 0x00, 0x02,
        0x00, 0x69, 0x73, 0x6f, 0x6d, 0x69, 0x73, 0x6f, 0x32, 0x61, 0x76, 0x63, 0x31, 0x6d, 0x70,
        0x34, 0x31,
    ]);

    // Step 1: Replace ftyp atom
    let mp4_data = fs::read(&tmp_mp4_0).context("Failed to read encoded MP4")?;
    let original_ftyp_size = read_box_header(&mp4_data, 0)
        .map(|(s, _)| s as usize)
        .context("Failed to read original ftyp size")?;
    let new_ftyp_size = ftyp_buffer.len();
    let ftyp_delta = new_ftyp_size as u64 - original_ftyp_size as u64;
    let mut mp4_step1 = replace_ftyp_box(&mp4_data, &ftyp_buffer)?;
    update_stco_offsets(&mut mp4_step1, ftyp_delta);
    fs::write(&tmp_mp4_1, &mp4_step1)?;

    // HTML wrapper
    let html_string = if let Some(html_path) = &args.html {
        let content = fs::read_to_string(html_path).context("Failed to read HTML")?;
        format!(
            "--><style>body{{font-size:0}}</style><div style=font-size:initial>{}</div><!--",
            content
        )
    } else {
        String::new()
    };
    let html_bytes = html_string.as_bytes();

    // Build skip atom
    let mut skip_payload = Vec::with_capacity(html_bytes.len() + png_data.len());
    skip_payload.extend_from_slice(html_bytes);
    skip_payload.extend_from_slice(&png_data);

    let skip_total = (skip_payload.len() + 8) as u32;
    let mut skip_buffer = Vec::with_capacity(skip_payload.len() + 8);
    skip_buffer.write_u32::<BigEndian>(skip_total)?;
    skip_buffer.extend_from_slice(b"skip");
    skip_buffer.extend_from_slice(&skip_payload);

    // Step 2: Insert skip atom after ftyp
    let skip_delta = skip_buffer.len() as u64;
    let mut mp4_step2 = insert_box_after_ftyp(&mp4_step1, &skip_buffer)?;
    update_stco_offsets(&mut mp4_step2, skip_delta);

    // Find PNG offset
    let skip_head = {
        let mut h = [0u8; 8];
        h[0..4].copy_from_slice(&skip_total.to_be_bytes());
        h[4..8].copy_from_slice(b"skip");
        h
    };
    let png_offset = find_subarray(&mp4_step2, &skip_head, 0)
        .map(|p| p + 8 + html_bytes.len())
        .context("Failed to find PNG offset")?;

    // Update ftyp buffer with final values
    ftyp_buffer[18..22].copy_from_slice(&(png_offset as u32).to_le_bytes());
    ftyp_buffer[4..8].copy_from_slice(&[1, 0, 0, 0]); // Image count = 1, clear atom name
    ftyp_buffer[240..256].copy_from_slice(b"isomiso2avc1mp41");

    // Extra data
    let extra_data = if let Some(extra_path) = &args.extra {
        fs::read(extra_path).context("Failed to read extra file")?
    } else {
        Vec::new()
    };
    let mut atom_free_addr = 22;
    ftyp_buffer[atom_free_addr..atom_free_addr + extra_data.len()].copy_from_slice(&extra_data);
    atom_free_addr += extra_data.len();
    ftyp_buffer[atom_free_addr..atom_free_addr + 4].copy_from_slice(b"<!--");
    atom_free_addr += 4;

    // PDF first pass
    let pdf_data = if let Some(pdf_path) = &args.pdf {
        Some(fs::read(pdf_path).context("Failed to read PDF")?)
    } else {
        None
    };
    let mp4_size = mp4_step2.len();

    if let Some(ref pdf_buf) = pdf_data {
        ftyp_buffer[atom_free_addr] = 0x0A;
        ftyp_buffer[atom_free_addr + 1..atom_free_addr + 10]
            .copy_from_slice(&pdf_buf[0..9.min(pdf_buf.len())]);
        atom_free_addr += 10;

        let mut offset = 30 + mp4_size.to_string().len();
        let obj_string;
        loop {
            offset -= 1;
            let len = mp4_size - atom_free_addr - extra_data.len() - offset;
            let candidate = format!("\n1 0 obj\n<</Length {}>>\nstream\n", len);
            if offset == candidate.len() {
                obj_string = candidate;
                break;
            }
        }
        let obj_bytes = obj_string.as_bytes();
        let end = atom_free_addr + extra_data.len() + obj_bytes.len();
        if end > ftyp_buffer.len() {
            bail!("PDF object exceeds ftyp buffer");
        }
        ftyp_buffer[atom_free_addr + extra_data.len()..end].copy_from_slice(obj_bytes);
    }

    // Step 3: Final ftyp replacement
    let mp4_final = replace_ftyp_box(&mp4_step2, &ftyp_buffer)?;

    // Write output
    let mut out = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&args.output)?;
    out.write_all(&mp4_final)?;

    // Bithack fix: zero byte 3 to split ftyp
    out.seek(SeekFrom::Start(3))?;
    out.write_all(&[0])?;
    drop(out);

    // PDF second pass
    if let Some(pdf_buf) = pdf_data {
        let term = b"\nendstream\nendobj\n";
        let mut final_pdf = vec![0u8; pdf_buf.len() + term.len() + 10];
        final_pdf[0..term.len()].copy_from_slice(term);
        final_pdf[term.len()..term.len() + pdf_buf.len()].copy_from_slice(&pdf_buf);

        let xref_pos = find_subarray(&final_pdf, b"\nxref", 0).context("Failed to find xref")? + 1;
        let sxref_pos = find_subarray(&final_pdf, b"\nstartxref", xref_pos)
            .context("Failed to find startxref")?
            + 1;

        // Parse xref: find the "0 383" line after "xref"
        // xref_pos points to 'x' in "xref"
        let mut scan = xref_pos;
        // Skip "xref"
        scan += 4;
        // Skip line ending
        if final_pdf.get(scan) == Some(&b'\r') {
            scan += 1;
        }
        if final_pdf.get(scan) == Some(&b'\n') {
            scan += 1;
        }
        // Now at "0 383"
        let line_end = final_pdf[scan..]
            .iter()
            .position(|&b| b == b'\n' || b == b'\r')
            .unwrap_or(20);
        let hdr = String::from_utf8_lossy(&final_pdf[scan..scan + line_end]);
        let parts: Vec<&str> = hdr.trim().split_whitespace().collect();
        let count: usize = parts
            .get(1)
            .and_then(|s| s.parse().ok())
            .context("Failed to parse xref count")?;

        // First entry line starts after this header line
        let mut curr = scan + line_end;
        if final_pdf.get(curr) == Some(&b'\r') {
            curr += 1;
        }
        if final_pdf.get(curr) == Some(&b'\n') {
            curr += 1;
        }

        let out_size = fs::metadata(&args.output)?.len() as usize;

        // Update all offset entries
        for _ in 0..count {
            if curr + 20 > final_pdf.len() {
                break;
            }
            let s = String::from_utf8_lossy(&final_pdf[curr..curr + 10]);
            if let Ok(off) = s.trim().parse::<usize>() {
                let new = pad_left(&(off + out_size + term.len()).to_string(), 10, '0');
                final_pdf[curr..curr + 10].copy_from_slice(new.as_bytes());
            }
            // Skip to next line (each entry is 20 bytes: 10 offset + space + 5 gen + space + 1 flag)
            curr = final_pdf[curr..]
                .iter()
                .position(|&b| b == b'\n')
                .map(|p| curr + p + 1)
                .unwrap_or(curr + 20);
        }

        // Adjust startxref offset
        let sxref_val_start = sxref_pos + 11;
        let sxref_val_end = final_pdf[sxref_val_start..]
            .iter()
            .position(|&b| b == b'\n' || b == b'\r')
            .map(|p| sxref_val_start + p)
            .unwrap_or(sxref_val_start + 10);
        let sxref_s = String::from_utf8_lossy(&final_pdf[sxref_val_start..sxref_val_end]);
        if let Ok(sxref) = sxref_s.trim().parse::<usize>() {
            let new_sxref = (sxref + out_size + term.len()).to_string();
            let nb = new_sxref.as_bytes();
            final_pdf[sxref_val_start..sxref_val_start + nb.len()].copy_from_slice(nb);
            let eof_pos = sxref_val_start + nb.len();
            if eof_pos + 7 <= final_pdf.len() {
                final_pdf[eof_pos..eof_pos + 7].copy_from_slice(b"\n%%EOF\n");
            }
            for b in final_pdf.iter_mut().skip(sxref_val_start + nb.len() + 17) {
                *b = 0;
            }
        }

        let mut out = fs::OpenOptions::new().append(true).open(&args.output)?;
        out.write_all(&final_pdf)?;
    }

    // Append appendables
    for path in &args.appendable {
        if !path.exists() {
            eprintln!("Warning: {} not found, skipping", path.display());
            continue;
        }
        let data = fs::read(path)?;
        let mut out = fs::OpenOptions::new().append(true).open(&args.output)?;
        out.write_all(&data)?;
    }

    // Handle ZIPs with pure Rust
    if !args.zip.is_empty() {
        let mut merged = Cursor::new(Vec::new());
        let mut zw = ZipWriter::new(&mut merged);
        let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

        let mut seen = std::collections::HashSet::new();
        for zp in &args.zip {
            let f = fs::File::open(zp).with_context(|| format!("Cannot open {}", zp.display()))?;
            let mut za = ZipArchive::new(f)?;
            for i in 0..za.len() {
                let mut entry = za.by_index(i)?;
                let name = entry.name().to_string();
                if seen.contains(&name) {
                    continue;
                }
                seen.insert(name.clone());
                zw.start_file(&name, opts)?;
                std::io::copy(&mut entry, &mut zw)?;
            }
        }
        zw.finish()?;
        let zip_bytes = merged.into_inner();

        let mut out = fs::OpenOptions::new().append(true).open(&args.output)?;
        out.write_all(&zip_bytes)?;

        // Run zip -A for offset fix (only external dep left)
        let exe = Path::new("deps/ffmpeg.exe");
        let zip_exe = exe.parent().unwrap().join("zip.exe");
        if zip_exe.exists() {
            let _ = Command::new(&zip_exe).arg("-A").arg(&args.output).output();
        }
    }

    println!("Polyglot created: {}", args.output.display());
    Ok(())
}
