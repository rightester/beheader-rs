use crate::mp4::box_ops::find_all_boxes;
use byteorder::{BigEndian, ReadBytesExt, WriteBytesExt};
use std::io::Cursor;

pub fn update_stco_offsets(mp4: &mut [u8], delta: u64) {
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
