// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! bzip2 decompression, in safe Rust and with no dependency: Cisco publishes OpenH264 as
//! `openh264-<version>-win64.dll.bz2`, and nothing else in the workspace reads bzip2.
//!
//! One stream, as `bzip2` writes it (block size 1 to 9, every block's CRC and the stream's
//! combined CRC checked). The obsolete "randomised" blocks are refused. `max_out` bounds
//! the output, so a hostile file cannot fill the disk or the memory.

pub fn decompress(input: &[u8], max_out: usize) -> Result<Vec<u8>, String> {
    let mut br = Bits { data: input, pos: 0, buf: 0, n: 0 };
    if br.bits(8)? != b'B' as u32 || br.bits(8)? != b'Z' as u32 || br.bits(8)? != b'h' as u32 {
        return Err("not a bzip2 file".into());
    }
    let level = br.bits(8)?;
    if !(b'1' as u32..=b'9' as u32).contains(&level) {
        return Err("bad bzip2 block size".into());
    }
    let max_block = (level - b'0' as u32) as usize * 100_000;
    let crcs = crc_table();
    let mut out = Vec::new();
    let mut combined = 0u32;
    loop {
        let (m1, m2) = (br.bits(24)?, br.bits(24)?);
        if (m1, m2) == (0x314159, 0x265359) {
            let want = br.bits32()?;
            let start = out.len();
            block(&mut br, max_block, max_out, &mut out)?;
            let mut c = !0u32;
            for &b in &out[start..] {
                c = (c << 8) ^ crcs[((c >> 24) as u8 ^ b) as usize];
            }
            let got = !c;
            if got != want {
                return Err("bzip2: block CRC mismatch".into());
            }
            combined = combined.rotate_left(1) ^ got;
        } else if (m1, m2) == (0x177245, 0x385090) {
            if br.bits32()? != combined {
                return Err("bzip2: stream CRC mismatch".into());
            }
            return Ok(out);
        } else {
            return Err("bzip2: bad block header".into());
        }
    }
}

struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
    buf: u64,
    n: u32,
}

impl Bits<'_> {
    /// The next `k` (at most 24) bits, most significant first.
    fn bits(&mut self, k: u32) -> Result<u32, String> {
        while self.n < k {
            let b = *self.data.get(self.pos).ok_or("bzip2: the file is cut short")?;
            self.pos += 1;
            self.buf = (self.buf << 8) | b as u64;
            self.n += 8;
        }
        self.n -= k;
        let v = ((self.buf >> self.n) & ((1u64 << k) - 1)) as u32;
        self.buf &= (1u64 << self.n) - 1;
        Ok(v)
    }
    fn bits32(&mut self) -> Result<u32, String> {
        Ok((self.bits(16)? << 16) | self.bits(16)?)
    }
}

fn crc_table() -> [u32; 256] {
    let mut t = [0u32; 256];
    for (i, e) in t.iter_mut().enumerate() {
        let mut c = (i as u32) << 24;
        for _ in 0..8 {
            c = if c & 0x8000_0000 != 0 { (c << 1) ^ 0x04c1_1db7 } else { c << 1 };
        }
        *e = c;
    }
    t
}

/// A canonical Huffman table, bzip2's decode layout.
struct Huff {
    min: usize,
    max: usize,
    limit: [i32; 23],
    base: [i32; 23],
    perm: Vec<u16>,
}

impl Huff {
    fn new(lens: &[u8]) -> Huff {
        let min = *lens.iter().min().unwrap() as usize;
        let max = *lens.iter().max().unwrap() as usize;
        let mut perm = Vec::with_capacity(lens.len());
        for l in min..=max {
            for (s, &sl) in lens.iter().enumerate() {
                if sl as usize == l {
                    perm.push(s as u16);
                }
            }
        }
        let mut base = [0i32; 23];
        for &l in lens {
            base[l as usize + 1] += 1;
        }
        for i in 1..23 {
            base[i] += base[i - 1];
        }
        let mut limit = [0i32; 23];
        let mut vec = 0i32;
        for i in min..=max {
            vec += base[i + 1] - base[i];
            limit[i] = vec - 1;
            vec <<= 1;
        }
        for i in min + 1..=max {
            base[i] = ((limit[i - 1] + 1) << 1) - base[i];
        }
        Huff { min, max, limit, base, perm }
    }

    fn decode(&self, br: &mut Bits) -> Result<usize, String> {
        let mut n = self.min;
        let mut v = br.bits(n as u32)? as i32;
        loop {
            if n > self.max {
                return Err("bzip2: bad Huffman code".into());
            }
            if v <= self.limit[n] {
                break;
            }
            n += 1;
            v = (v << 1) | br.bits(1)? as i32;
        }
        let at = v - self.base[n];
        self.perm.get(at as usize).map(|&s| s as usize).ok_or_else(|| "bzip2: bad Huffman code".into())
    }
}

/// One block (after its magic and CRC): appends the decoded bytes to `out`.
fn block(br: &mut Bits, max_block: usize, max_out: usize, out: &mut Vec<u8>) -> Result<(), String> {
    if br.bits(1)? != 0 {
        return Err("bzip2: randomised blocks are not supported".into());
    }
    let orig_ptr = br.bits(24)? as usize;
    // Which byte values the block uses.
    let used16 = br.bits(16)?;
    let mut unseq: Vec<u8> = Vec::new();
    for i in 0..16 {
        if used16 & (0x8000 >> i) != 0 {
            let m = br.bits(16)?;
            for j in 0..16 {
                if m & (0x8000 >> j) != 0 {
                    unseq.push((i * 16 + j) as u8);
                }
            }
        }
    }
    if unseq.is_empty() {
        return Err("bzip2: a block with no symbols".into());
    }
    let alpha = unseq.len() + 2;
    let eob = alpha - 1;
    let n_groups = br.bits(3)? as usize;
    if !(2..=6).contains(&n_groups) {
        return Err("bzip2: bad table count".into());
    }
    let n_sel = br.bits(15)? as usize;
    if n_sel == 0 {
        return Err("bzip2: no selectors".into());
    }
    let mut pos: Vec<u8> = (0..n_groups as u8).collect();
    let mut selectors = Vec::with_capacity(n_sel);
    for _ in 0..n_sel {
        let mut j = 0;
        while br.bits(1)? == 1 {
            j += 1;
            if j >= n_groups {
                return Err("bzip2: bad selector".into());
            }
        }
        let v = pos.remove(j);
        pos.insert(0, v);
        selectors.push(v as usize);
    }
    let mut tables = Vec::with_capacity(n_groups);
    for _ in 0..n_groups {
        let mut len = br.bits(5)? as i32;
        let mut lens = vec![0u8; alpha];
        for l in lens.iter_mut() {
            loop {
                if !(1..=20).contains(&len) {
                    return Err("bzip2: bad code length".into());
                }
                if br.bits(1)? == 0 {
                    break;
                }
                len += if br.bits(1)? == 0 { 1 } else { -1 };
            }
            *l = len as u8;
        }
        tables.push(Huff::new(&lens));
    }

    // Huffman, then run-length (RUNA/RUNB) and move-to-front decoding into `tt`.
    let mut mtf: [u8; 256] = std::array::from_fn(|i| i as u8);
    let mut tt: Vec<u32> = Vec::new();
    let mut counts = [0u32; 256];
    let (mut run, mut weight) = (0usize, 1usize);
    let (mut group, mut left) = (0usize, 0usize);
    let mut cur = &tables[0];
    loop {
        if left == 0 {
            let s = *selectors.get(group).ok_or("bzip2: out of selectors")?;
            cur = &tables[s];
            group += 1;
            left = 50;
        }
        left -= 1;
        let sym = cur.decode(br)?;
        if sym <= 1 {
            run += weight << sym;
            weight <<= 1;
            if weight > 1 << 22 {
                return Err("bzip2: run too long".into());
            }
            continue;
        }
        if run > 0 {
            let byte = unseq[mtf[0] as usize];
            if tt.len() + run > max_block {
                return Err("bzip2: block too big".into());
            }
            counts[byte as usize] += run as u32;
            tt.resize(tt.len() + run, byte as u32);
            run = 0;
            weight = 1;
        }
        if sym == eob {
            break;
        }
        let idx = sym - 1;
        if idx >= unseq.len() {
            return Err("bzip2: bad symbol".into());
        }
        let v = mtf[idx];
        mtf.copy_within(0..idx, 1);
        mtf[0] = v;
        let byte = unseq[v as usize];
        if tt.len() >= max_block {
            return Err("bzip2: block too big".into());
        }
        counts[byte as usize] += 1;
        tt.push(byte as u32);
    }
    let n = tt.len();
    if n == 0 || orig_ptr >= n {
        return Err("bzip2: bad block".into());
    }

    // Undo the Burrows-Wheeler transform.
    let mut cf = [0u32; 256];
    let mut sum = 0;
    for i in 0..256 {
        cf[i] = sum;
        sum += counts[i];
    }
    for i in 0..n {
        let b = (tt[i] & 0xff) as usize;
        let j = cf[b] as usize;
        tt[j] |= (i as u32) << 8;
        cf[b] += 1;
    }
    // And the first run-length stage: four equal bytes are followed by a repeat count.
    let mut t = tt[orig_ptr] >> 8;
    let (mut prev, mut same) = (-1i32, 0u32);
    for _ in 0..n {
        let e = tt[t as usize];
        let ch = (e & 0xff) as u8;
        t = e >> 8;
        if same == 4 {
            if out.len() + ch as usize > max_out {
                return Err("bzip2: output too big".into());
            }
            out.resize(out.len() + ch as usize, prev as u8);
            same = 0;
            prev = -1;
            continue;
        }
        if ch as i32 == prev {
            same += 1;
        } else {
            same = 1;
            prev = ch as i32;
        }
        if out.len() >= max_out {
            return Err("bzip2: output too big".into());
        }
        out.push(ch);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows() -> Vec<u8> {
        (0..40_000).flat_map(|i| format!("row {}\n", i % 997).into_bytes()).collect()
    }

    fn runs() -> Vec<u8> {
        let mut v = vec![];
        for n in [1usize, 3, 4, 5, 6, 255, 256, 259, 260, 300, 1000, 70_000] {
            v.extend(std::iter::repeat_n((n % 251) as u8, n));
        }
        v
    }

    #[test]
    fn decodes_what_bzip2_wrote() {
        let big = 1 << 22;
        assert_eq!(decompress(include_bytes!("../tests/data/hello.bz2"), big).unwrap(), b"hello, bzip2\n");
        // Three blocks of 100 kB (bzip2 -1), then long runs on every side of the 4-byte rule.
        assert_eq!(decompress(include_bytes!("../tests/data/rows.bz2"), big).unwrap(), rows());
        assert_eq!(decompress(include_bytes!("../tests/data/runs.bz2"), big).unwrap(), runs());
    }

    #[test]
    fn refuses_damage_and_oversize() {
        let good = include_bytes!("../tests/data/rows.bz2");
        let mut bad = good.to_vec();
        bad[good.len() / 2] ^= 0x10;
        assert!(decompress(&bad, 1 << 22).is_err());
        assert!(decompress(&good[..good.len() - 9], 1 << 22).is_err());
        assert!(decompress(good, 1000).is_err(), "the output cap");
        assert!(decompress(b"PK\x03\x04", 10).is_err());
    }
}
