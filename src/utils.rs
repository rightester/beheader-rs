use byteorder::{BigEndian, ReadBytesExt};
use std::io::Cursor;

pub fn find_subarray(haystack: &[u8], needle: &[u8], start: usize) -> Option<usize> {
    if needle.is_empty() || start >= haystack.len() {
        return None;
    }
    haystack[start..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|i| start + i)
}

pub fn pad_left(s: &str, target_len: usize, pad_char: char) -> String {
    if s.len() >= target_len {
        s.to_string()
    } else {
        format!("{}{}", pad_char.to_string().repeat(target_len - s.len()), s)
    }
}

pub fn read_box_header(data: &[u8], offset: usize) -> Option<(u64, [u8; 4])> {
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
