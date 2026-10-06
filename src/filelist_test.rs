use super::*;

fn write_long(out: &mut Vec<u8>, value: i64) {
    let mut zig = ((value << 1) ^ (value >> 63)) as u64;
    loop {
        let mut byte = (zig & 0x7f) as u8;
        zig >>= 7;
        if zig != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if zig == 0 {
            break;
        }
    }
}

fn write_string(out: &mut Vec<u8>, value: &str) {
    write_long(out, value.len() as i64);
    out.extend_from_slice(value.as_bytes());
}

#[test]
fn decodes_single_block() {
    let mut buf = Vec::new();
    write_long(&mut buf, 2);
    for (path, hash, size, mtime) in [
        ("/download/aa", "aaa", 10i64, 1i64),
        ("/download/bb", "bbb", 20, 2),
    ] {
        write_string(&mut buf, path);
        write_string(&mut buf, hash);
        write_long(&mut buf, size);
        write_long(&mut buf, mtime);
    }
    write_long(&mut buf, 0);
    let files = decode(&buf).unwrap();
    assert_eq!(files.len(), 2);
    assert_eq!(files[0].path, "/download/aa");
    assert_eq!(files[1].size, 20);
}

#[test]
fn decodes_negative_block() {
    let mut buf = Vec::new();
    write_long(&mut buf, -1);
    write_long(&mut buf, 99); // block byte size (ignored)
    write_string(&mut buf, "/download/cc");
    write_string(&mut buf, "ccc");
    write_long(&mut buf, 30);
    write_long(&mut buf, 3);
    write_long(&mut buf, 0);
    let files = decode(&buf).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].hash, "ccc");
}
