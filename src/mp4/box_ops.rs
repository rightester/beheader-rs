use crate::utils::{find_subarray, read_box_header};
use anyhow::{Context, Result};

pub fn find_boxes_recursive(
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
                find_boxes_recursive(data, p + 8, p + size as usize, target, results);
            }
            p += size as usize;
        } else {
            break;
        }
    }
}

pub fn find_all_boxes(data: &[u8], target: &[u8; 4]) -> Vec<(u64, usize)> {
    let mut results = Vec::new();
    find_boxes_recursive(data, 0, data.len(), target, &mut results);
    results
}

pub fn replace_ftyp_box(mp4: &[u8], new_ftyp: &[u8]) -> Result<Vec<u8>> {
    let (ftyp_size, _) =
        read_box_header(mp4, 0).with_context(|| "Failed to read ftyp box header")?;
    let ftyp_end = ftyp_size as usize;
    let mut result = Vec::with_capacity(mp4.len() - ftyp_end + new_ftyp.len());
    result.extend_from_slice(new_ftyp);
    result.extend_from_slice(&mp4[ftyp_end..]);
    Ok(result)
}

pub fn insert_box_after_ftyp(mp4: &[u8], new_box: &[u8]) -> Result<Vec<u8>> {
    let (ftyp_size, _) =
        read_box_header(mp4, 0).with_context(|| "Failed to read ftyp box header")?;
    let ftyp_end = ftyp_size as usize;
    let mut result = Vec::with_capacity(mp4.len() + new_box.len());
    result.extend_from_slice(&mp4[..ftyp_end]);
    result.extend_from_slice(new_box);
    result.extend_from_slice(&mp4[ftyp_end..]);
    Ok(result)
}

pub fn build_skip_box(html_bytes: &[u8], png_data: &[u8]) -> std::io::Result<Vec<u8>> {
    use byteorder::{BigEndian, WriteBytesExt};

    let mut skip_payload = Vec::with_capacity(html_bytes.len() + png_data.len());
    skip_payload.extend_from_slice(html_bytes);
    skip_payload.extend_from_slice(png_data);

    let skip_total = (skip_payload.len() + 8) as u32;
    let mut skip_buffer = Vec::with_capacity(skip_payload.len() + 8);
    skip_buffer.write_u32::<BigEndian>(skip_total)?;
    skip_buffer.extend_from_slice(b"skip");
    skip_buffer.extend_from_slice(&skip_payload);

    Ok(skip_buffer)
}

pub fn find_png_offset(mp4_data: &[u8], html_bytes: &[u8], png_data: &[u8]) -> Option<usize> {
    let skip_total = (html_bytes.len() + png_data.len() + 8) as u32;
    let mut skip_head = [0u8; 8];
    skip_head[0..4].copy_from_slice(&skip_total.to_be_bytes());
    skip_head[4..8].copy_from_slice(b"skip");

    find_subarray(mp4_data, &skip_head, 0).map(|p| p + 8 + html_bytes.len())
}
