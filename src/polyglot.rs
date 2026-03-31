use anyhow::{bail, Context, Result};
use std::io::{Cursor, Seek, SeekFrom, Write};
use std::path::Path;

use crate::mp4::{
    build_skip_box, find_png_offset, insert_box_after_ftyp, replace_ftyp_box, update_stco_offsets,
};
use crate::utils::{find_subarray, pad_left, read_box_header};

pub struct PolyglotConfig {
    pub png_data: Vec<u8>,
    pub mp4_data: Vec<u8>,
    pub html_content: Option<String>,
    pub pdf_data: Option<Vec<u8>>,
    pub extra_data: Option<Vec<u8>>,
}

pub struct PolyglotResult {
    pub data: Vec<u8>,
    pub pdf_suffix: Option<Vec<u8>>,
}

pub fn build_polyglot(config: &PolyglotConfig) -> Result<PolyglotResult> {
    let html_bytes = if let Some(ref html) = config.html_content {
        format!(
            "--><style>body{{font-size:0}}</style><div style=font-size:initial>{}</div><!--",
            html
        )
        .into_bytes()
    } else {
        Vec::new()
    };

    let extra_data = config.extra_data.clone().unwrap_or_default();

    let mut ftyp_buffer = vec![0u8; 288];
    ftyp_buffer[2] = 1;
    ftyp_buffer[4..8].copy_from_slice(b"ftyp");
    ftyp_buffer[3] = 32;
    ftyp_buffer[12] = 32;
    ftyp_buffer[14..18].copy_from_slice(&(config.png_data.len() as u32).to_le_bytes());
    ftyp_buffer[256..288].copy_from_slice(&[
        0x00, 0x00, 0x00, 0x20, 0x66, 0x74, 0x79, 0x70, 0x69, 0x73, 0x6f, 0x6d, 0x00, 0x00, 0x02,
        0x00, 0x69, 0x73, 0x6f, 0x6d, 0x69, 0x73, 0x6f, 0x32, 0x61, 0x76, 0x63, 0x31, 0x6d, 0x70,
        0x34, 0x31,
    ]);

    let mp4_data = &config.mp4_data;
    let original_ftyp_size = read_box_header(mp4_data, 0)
        .map(|(s, _)| s as usize)
        .context("Failed to read original ftyp size")?;
    let new_ftyp_size = ftyp_buffer.len();
    let ftyp_delta = new_ftyp_size as u64 - original_ftyp_size as u64;

    let mut mp4_step1 = replace_ftyp_box(mp4_data, &ftyp_buffer)?;
    update_stco_offsets(&mut mp4_step1, ftyp_delta);

    let skip_buffer = build_skip_box(&html_bytes, &config.png_data)?;
    let skip_delta = skip_buffer.len() as u64;
    let mut mp4_step2 = insert_box_after_ftyp(&mp4_step1, &skip_buffer)?;
    update_stco_offsets(&mut mp4_step2, skip_delta);

    let png_offset = find_png_offset(&mp4_step2, &html_bytes, &config.png_data)
        .context("Failed to find PNG offset")?;

    ftyp_buffer[18..22].copy_from_slice(&(png_offset as u32).to_le_bytes());
    ftyp_buffer[4..8].copy_from_slice(&[1, 0, 0, 0]);
    ftyp_buffer[240..256].copy_from_slice(b"isomiso2avc1mp41");

    let mut atom_free_addr = 22;
    ftyp_buffer[atom_free_addr..atom_free_addr + extra_data.len()].copy_from_slice(&extra_data);
    atom_free_addr += extra_data.len();
    ftyp_buffer[atom_free_addr..atom_free_addr + 4].copy_from_slice(b"<!--");
    atom_free_addr += 4;

    if let Some(ref pdf_buf) = config.pdf_data {
        ftyp_buffer[atom_free_addr] = 0x0A;
        ftyp_buffer[atom_free_addr + 1..atom_free_addr + 10]
            .copy_from_slice(&pdf_buf[0..9.min(pdf_buf.len())]);
        atom_free_addr += 10;

        let mp4_size = mp4_step2.len();
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

    let mp4_final = replace_ftyp_box(&mp4_step2, &ftyp_buffer)?;

    let mut output = Cursor::new(Vec::with_capacity(mp4_final.len() + 1));
    output.write_all(&mp4_final)?;
    output.seek(SeekFrom::Start(3))?;
    output.write_all(&[0])?;

    let pdf_suffix = if let Some(ref pdf_buf) = config.pdf_data {
        Some(build_pdf_suffix(pdf_buf, output.get_ref().len())?)
    } else {
        None
    };

    Ok(PolyglotResult {
        data: output.into_inner(),
        pdf_suffix,
    })
}

fn build_pdf_suffix(pdf_buf: &[u8], out_size: usize) -> Result<Vec<u8>> {
    let term = b"\nendstream\nendobj\n";
    let mut final_pdf = vec![0u8; pdf_buf.len() + term.len() + 10];
    final_pdf[0..term.len()].copy_from_slice(term);
    final_pdf[term.len()..term.len() + pdf_buf.len()].copy_from_slice(pdf_buf);

    let xref_pos = find_subarray(&final_pdf, b"\nxref", 0).context("Failed to find xref")? + 1;
    let sxref_pos = find_subarray(&final_pdf, b"\nstartxref", xref_pos)
        .context("Failed to find startxref")?
        + 1;

    let mut scan = xref_pos + 4;
    if final_pdf.get(scan) == Some(&b'\r') {
        scan += 1;
    }
    if final_pdf.get(scan) == Some(&b'\n') {
        scan += 1;
    }

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

    let mut curr = scan + line_end;
    if final_pdf.get(curr) == Some(&b'\r') {
        curr += 1;
    }
    if final_pdf.get(curr) == Some(&b'\n') {
        curr += 1;
    }

    for _ in 0..count {
        if curr + 20 > final_pdf.len() {
            break;
        }
        let s = String::from_utf8_lossy(&final_pdf[curr..curr + 10]);
        if let Ok(off) = s.trim().parse::<usize>() {
            let new = pad_left(&(off + out_size + term.len()).to_string(), 10, '0');
            final_pdf[curr..curr + 10].copy_from_slice(new.as_bytes());
        }
        curr = final_pdf[curr..]
            .iter()
            .position(|&b| b == b'\n')
            .map(|p| curr + p + 1)
            .unwrap_or(curr + 20);
    }

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

    Ok(final_pdf)
}

pub fn append_zip_to_output(output_path: &Path, zip_paths: &[impl AsRef<Path>]) -> Result<()> {
    use std::collections::HashSet;
    use std::fs;
    use zip::write::SimpleFileOptions;
    use zip::{CompressionMethod, ZipArchive, ZipWriter};

    let mut merged = Cursor::new(Vec::new());
    let mut zw = ZipWriter::new(&mut merged);
    let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    let mut seen = HashSet::new();
    for zp in zip_paths {
        let f =
            fs::File::open(zp).with_context(|| format!("Cannot open {}", zp.as_ref().display()))?;
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

    let mut out = fs::OpenOptions::new().append(true).open(output_path)?;
    out.write_all(&zip_bytes)?;

    Ok(())
}
