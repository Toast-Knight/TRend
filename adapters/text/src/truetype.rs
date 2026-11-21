//! A small TrueType reader: enough to draw text. Reads the character map (formats 4 and 12),
//! horizontal metrics, simple and composite glyph outlines (quadratic curves, the `glyf` table) and
//! pair kerning from the `kern` table. Fonts with CFF outlines (most `.otf` files) are refused.
//! Every offset is checked; a damaged font is an error, never a panic.

use std::collections::HashMap;
use std::fmt;

/// Why a font could not be read.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum FontError {
    /// Not a TrueType font or collection.
    NotTrueType,
    /// A table the reader needs is missing.
    MissingTable(&'static str),
    /// PostScript (CFF) outlines, as in most `.otf` files, are not supported.
    CffOutlines,
    /// The font is damaged: a table points outside the file.
    Damaged(&'static str),
}

impl fmt::Display for FontError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotTrueType => formatter.write_str("not a TrueType font"),
            Self::MissingTable(table) => write!(formatter, "font has no '{table}' table"),
            Self::CffOutlines => {
                formatter.write_str("font has PostScript (CFF) outlines, which are not supported")
            }
            Self::Damaged(what) => write!(formatter, "damaged font: {what}"),
        }
    }
}

impl std::error::Error for FontError {}

fn u16_at(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*data.get(at)?, *data.get(at + 1)?]))
}

fn i16_at(data: &[u8], at: usize) -> Option<i16> {
    u16_at(data, at).map(|value| value as i16)
}

fn u32_at(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes([
        *data.get(at)?,
        *data.get(at + 1)?,
        *data.get(at + 2)?,
        *data.get(at + 3)?,
    ]))
}

/// One step of a glyph outline, in font units (y up).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum PathOp {
    Move(f32, f32),
    Line(f32, f32),
    Quad(f32, f32, f32, f32),
    Close,
}

/// A parsed TrueType font.
pub(crate) struct TrueType {
    data: Vec<u8>,
    glyf: usize,
    glyf_len: usize,
    loca: usize,
    long_loca: bool,
    hmtx: usize,
    metric_count: u16,
    glyph_count: u16,
    pub(crate) units_per_em: f32,
    pub(crate) ascender: f32,
    pub(crate) descender: f32,
    cmap: Cmap,
    kerning: HashMap<u32, i16>,
}

enum Cmap {
    /// Format 4 subtable at this offset.
    Segments(usize),
    /// Format 12 groups: (first, last, first glyph).
    Groups(Vec<(u32, u32, u32)>),
}

impl TrueType {
    /// Reads font `index` of `data` (0 for a plain `.ttf`; any font of a `.ttc` collection).
    pub(crate) fn parse(data: Vec<u8>, index: u32) -> Result<Self, FontError> {
        let damaged = FontError::Damaged;
        let start = if data.starts_with(b"ttcf") {
            let count = u32_at(&data, 8).ok_or(damaged("collection header"))?;
            if index >= count {
                return Err(damaged("collection index"));
            }
            u32_at(&data, 12 + index as usize * 4).ok_or(damaged("collection header"))? as usize
        } else {
            0
        };
        match u32_at(&data, start) {
            Some(0x0001_0000 | 0x7472_7565) => {}
            Some(0x4F54_544F) => return Err(FontError::CffOutlines),
            _ => return Err(FontError::NotTrueType),
        }
        let table_count = u16_at(&data, start + 4).ok_or(damaged("header"))? as usize;
        let mut tables: HashMap<[u8; 4], (usize, usize)> = HashMap::new();
        for table in 0..table_count {
            let record = start + 12 + table * 16;
            let tag = data
                .get(record..record + 4)
                .ok_or(damaged("table directory"))?;
            let offset = u32_at(&data, record + 8).ok_or(damaged("table directory"))? as usize;
            let length = u32_at(&data, record + 12).ok_or(damaged("table directory"))? as usize;
            if offset
                .checked_add(length)
                .is_none_or(|end| end > data.len())
            {
                return Err(damaged("table outside the file"));
            }
            tables.insert([tag[0], tag[1], tag[2], tag[3]], (offset, length));
        }
        let table = |tag: &'static str| -> Result<(usize, usize), FontError> {
            let bytes = tag.as_bytes();
            tables
                .get(&[bytes[0], bytes[1], bytes[2], bytes[3]])
                .copied()
                .ok_or(FontError::MissingTable(tag))
        };
        if table("glyf").is_err() && table("CFF ").is_ok() {
            return Err(FontError::CffOutlines);
        }
        let (head, _) = table("head")?;
        let (hhea, _) = table("hhea")?;
        let (maxp, _) = table("maxp")?;
        let (hmtx, _) = table("hmtx")?;
        let (loca, _) = table("loca")?;
        let (glyf, glyf_len) = table("glyf")?;
        let (cmap, cmap_len) = table("cmap")?;
        let units_per_em = f32::from(u16_at(&data, head + 18).ok_or(damaged("head"))?.max(16));
        let long_loca = i16_at(&data, head + 50).ok_or(damaged("head"))? != 0;
        let ascender = f32::from(i16_at(&data, hhea + 4).ok_or(damaged("hhea"))?);
        let descender = f32::from(i16_at(&data, hhea + 6).ok_or(damaged("hhea"))?);
        let metric_count = u16_at(&data, hhea + 34).ok_or(damaged("hhea"))?.max(1);
        let glyph_count = u16_at(&data, maxp + 4).ok_or(damaged("maxp"))?;
        let cmap = Self::read_cmap(&data, cmap, cmap_len).ok_or(damaged("cmap"))?;
        let kerning = table("kern")
            .ok()
            .and_then(|(offset, _)| Self::read_kern(&data, offset))
            .unwrap_or_default();
        Ok(Self {
            data,
            glyf,
            glyf_len,
            loca,
            long_loca,
            hmtx,
            metric_count,
            glyph_count,
            units_per_em,
            ascender,
            descender,
            cmap,
            kerning,
        })
    }

    fn read_cmap(data: &[u8], offset: usize, length: usize) -> Option<Cmap> {
        let count = u16_at(data, offset + 2)? as usize;
        // Prefer full Unicode (format 12), then the Basic Multilingual Plane (format 4).
        let mut best: Option<(u8, usize)> = None;
        for index in 0..count {
            let record = offset + 4 + index * 8;
            let platform = u16_at(data, record)?;
            let encoding = u16_at(data, record + 2)?;
            let subtable = offset + u32_at(data, record + 4)? as usize;
            if subtable >= offset + length {
                continue;
            }
            let format = u16_at(data, subtable)?;
            let unicode = platform == 0 || (platform == 3 && matches!(encoding, 1 | 10));
            let rank = match format {
                12 if unicode => 2,
                4 if unicode => 1,
                _ => continue,
            };
            if best.is_none_or(|(current, _)| rank > current) {
                best = Some((rank, subtable));
            }
        }
        let (rank, subtable) = best?;
        if rank == 1 {
            return Some(Cmap::Segments(subtable));
        }
        let groups = u32_at(data, subtable + 12)? as usize;
        let mut out = Vec::with_capacity(groups.min(65_536));
        for group in 0..groups.min(1 << 20) {
            let at = subtable + 16 + group * 12;
            out.push((
                u32_at(data, at)?,
                u32_at(data, at + 4)?,
                u32_at(data, at + 8)?,
            ));
        }
        Some(Cmap::Groups(out))
    }

    fn read_kern(data: &[u8], offset: usize) -> Option<HashMap<u32, i16>> {
        let tables = u16_at(data, offset + 2)?;
        let mut pairs = HashMap::new();
        let mut at = offset + 4;
        for _ in 0..tables {
            let length = u16_at(data, at + 2)? as usize;
            let coverage = u16_at(data, at + 4)?;
            // Format 0, horizontal, not cross-stream or minimum values.
            if coverage >> 8 == 0 && coverage & 0x7 == 1 {
                let count = u16_at(data, at + 6)? as usize;
                for pair in 0..count.min(1 << 16) {
                    let record = at + 14 + pair * 6;
                    pairs.insert(u32_at(data, record)?, i16_at(data, record + 4)?);
                }
            }
            at += length.max(6);
        }
        Some(pairs)
    }

    /// The glyph for `character`, 0 (the font's "missing" glyph) if it has none.
    pub(crate) fn glyph(&self, character: char) -> u16 {
        let code = u32::from(character);
        let data = &self.data[..];
        let found = match &self.cmap {
            Cmap::Groups(groups) => groups
                .binary_search_by(|&(first, last, _)| {
                    if code < first {
                        std::cmp::Ordering::Greater
                    } else if code > last {
                        std::cmp::Ordering::Less
                    } else {
                        std::cmp::Ordering::Equal
                    }
                })
                .ok()
                .map(|index| {
                    let (first, _, glyph) = groups[index];
                    glyph + (code - first)
                }),
            &Cmap::Segments(subtable) => (|| {
                if code > 0xFFFF {
                    return None;
                }
                let segments = u16_at(data, subtable + 6)? as usize / 2;
                let ends = subtable + 14;
                let starts = ends + segments * 2 + 2;
                let deltas = starts + segments * 2;
                let ranges = deltas + segments * 2;
                for segment in 0..segments {
                    let end = u32::from(u16_at(data, ends + segment * 2)?);
                    if code > end {
                        continue;
                    }
                    let start = u32::from(u16_at(data, starts + segment * 2)?);
                    if code < start {
                        return None;
                    }
                    let delta = u32::from(u16_at(data, deltas + segment * 2)?);
                    let range_at = ranges + segment * 2;
                    let range = u16_at(data, range_at)? as usize;
                    if range == 0 {
                        return Some((code + delta) & 0xFFFF);
                    }
                    let glyph = u32::from(u16_at(
                        data,
                        range_at + range + (code - start) as usize * 2,
                    )?);
                    return (glyph != 0).then_some((glyph + delta) & 0xFFFF);
                }
                None
            })(),
        };
        found
            .filter(|&glyph| glyph < u32::from(self.glyph_count))
            .map_or(0, |glyph| glyph as u16)
    }

    /// Advance width in font units.
    pub(crate) fn advance(&self, glyph: u16) -> f32 {
        let index = glyph.min(self.metric_count - 1) as usize;
        f32::from(u16_at(&self.data, self.hmtx + index * 4).unwrap_or(0))
    }

    /// Kerning between two glyphs in font units (negative pulls them together).
    pub(crate) fn kerning(&self, left: u16, right: u16) -> f32 {
        if self.kerning.is_empty() {
            return 0.0;
        }
        let key = (u32::from(left) << 16) | u32::from(right);
        self.kerning
            .get(&key)
            .map_or(0.0, |&value| f32::from(value))
    }

    fn glyph_range(&self, glyph: u16) -> Option<(usize, usize)> {
        if glyph >= self.glyph_count {
            return None;
        }
        let index = glyph as usize;
        let (start, end) = if self.long_loca {
            (
                u32_at(&self.data, self.loca + index * 4)? as usize,
                u32_at(&self.data, self.loca + index * 4 + 4)? as usize,
            )
        } else {
            (
                u16_at(&self.data, self.loca + index * 2)? as usize * 2,
                u16_at(&self.data, self.loca + index * 2 + 2)? as usize * 2,
            )
        };
        (start <= end && end <= self.glyf_len).then_some((self.glyf + start, end - start))
    }

    /// The outline of `glyph` in font units, empty for spaces or damaged glyphs.
    pub(crate) fn outline(&self, glyph: u16) -> Vec<PathOp> {
        let mut path = Vec::new();
        self.append_outline(glyph, [1.0, 0.0, 0.0, 1.0, 0.0, 0.0], 0, &mut path);
        path
    }

    /// Appends `glyph`'s outline transformed by `[a, b, c, d, dx, dy]`
    /// (`x' = a·x + c·y + dx`, `y' = b·x + d·y + dy`).
    fn append_outline(&self, glyph: u16, transform: [f32; 6], depth: u8, path: &mut Vec<PathOp>) {
        if depth > 8 {
            return;
        }
        let Some((start, length)) = self.glyph_range(glyph) else {
            return;
        };
        if length < 10 {
            return;
        }
        let data = &self.data[..start + length];
        let Some(contours) = i16_at(data, start) else {
            return;
        };
        if contours >= 0 {
            let _ = self.simple_outline(data, start, contours as usize, transform, path);
        } else {
            let _ = self.composite_outline(data, start + 10, transform, depth, path);
        }
    }

    fn simple_outline(
        &self,
        data: &[u8],
        start: usize,
        contours: usize,
        transform: [f32; 6],
        path: &mut Vec<PathOp>,
    ) -> Option<()> {
        let mut ends = Vec::with_capacity(contours);
        for contour in 0..contours {
            ends.push(u16_at(data, start + 10 + contour * 2)? as usize);
        }
        let points = ends.last().map_or(0, |&end| end + 1);
        if points > 0xFFFF {
            return None;
        }
        let instructions = u16_at(data, start + 10 + contours * 2)? as usize;
        let mut at = start + 12 + contours * 2 + instructions;
        let mut flags = Vec::with_capacity(points);
        while flags.len() < points {
            let flag = *data.get(at)?;
            at += 1;
            flags.push(flag);
            if flag & 8 != 0 {
                let repeat = *data.get(at)?;
                at += 1;
                for _ in 0..repeat {
                    flags.push(flag);
                }
            }
        }
        flags.truncate(points);
        let mut read_axis = |short: u8, same: u8| -> Option<Vec<f32>> {
            let mut values = Vec::with_capacity(points);
            let mut value = 0_i32;
            for &flag in &flags {
                if flag & short != 0 {
                    let delta = i32::from(*data.get(at)?);
                    at += 1;
                    value += if flag & same != 0 { delta } else { -delta };
                } else if flag & same == 0 {
                    value += i32::from(i16_at(data, at)?);
                    at += 2;
                }
                values.push(value as f32);
            }
            Some(values)
        };
        let xs = read_axis(2, 16)?;
        let ys = read_axis(4, 32)?;
        let [a, b, c, d, dx, dy] = transform;
        let point = |index: usize| -> (f32, f32, bool) {
            let (x, y) = (xs[index], ys[index]);
            (
                a * x + c * y + dx,
                b * x + d * y + dy,
                flags[index] & 1 != 0,
            )
        };
        let mut first = 0;
        for &end in &ends {
            if end < first || end >= points {
                return None;
            }
            let count = end - first + 1;
            if count >= 2 {
                let contour: Vec<(f32, f32, bool)> = (first..=end).map(point).collect();
                trace_contour(&contour, path);
            }
            first = end + 1;
        }
        Some(())
    }

    fn composite_outline(
        &self,
        data: &[u8],
        mut at: usize,
        transform: [f32; 6],
        depth: u8,
        path: &mut Vec<PathOp>,
    ) -> Option<()> {
        loop {
            let flags = u16_at(data, at)?;
            let component = u16_at(data, at + 2)?;
            at += 4;
            let (dx, dy) = if flags & 1 != 0 {
                let values = (i16_at(data, at)?, i16_at(data, at + 2)?);
                at += 4;
                (f32::from(values.0), f32::from(values.1))
            } else {
                let values = (*data.get(at)? as i8, *data.get(at + 1)? as i8);
                at += 2;
                (f32::from(values.0), f32::from(values.1))
            };
            // Point-matched placement (flag 2 unset) is rare; place those at the origin.
            let (dx, dy) = if flags & 2 != 0 { (dx, dy) } else { (0.0, 0.0) };
            let f2dot14 = |at: usize| i16_at(data, at).map(|value| f32::from(value) / 16_384.0);
            let (mut a, mut b, mut c, mut d) = (1.0, 0.0, 0.0, 1.0);
            if flags & 8 != 0 {
                a = f2dot14(at)?;
                d = a;
                at += 2;
            } else if flags & 0x40 != 0 {
                a = f2dot14(at)?;
                d = f2dot14(at + 2)?;
                at += 4;
            } else if flags & 0x80 != 0 {
                a = f2dot14(at)?;
                b = f2dot14(at + 2)?;
                c = f2dot14(at + 4)?;
                d = f2dot14(at + 6)?;
                at += 8;
            }
            let [pa, pb, pc, pd, pdx, pdy] = transform;
            let combined = [
                pa * a + pc * b,
                pb * a + pd * b,
                pa * c + pc * d,
                pb * c + pd * d,
                pa * dx + pc * dy + pdx,
                pb * dx + pd * dy + pdy,
            ];
            self.append_outline(component, combined, depth + 1, path);
            if flags & 0x20 == 0 {
                return Some(());
            }
        }
    }
}

/// Turns one contour of on- and off-curve points into path steps.
fn trace_contour(points: &[(f32, f32, bool)], path: &mut Vec<PathOp>) {
    let count = points.len();
    let midpoint =
        |a: (f32, f32, bool), b: (f32, f32, bool)| ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
    // Start on an on-curve point, or between two off-curve points.
    let (start, offset) = match points.iter().position(|point| point.2) {
        Some(index) => ((points[index].0, points[index].1), index),
        None => (midpoint(points[0], points[1]), 0),
    };
    path.push(PathOp::Move(start.0, start.1));
    let mut control: Option<(f32, f32)> = None;
    for step in 1..=count {
        let point = points[(offset + step) % count];
        let here = (point.0, point.1);
        match (point.2, control) {
            (true, None) => path.push(PathOp::Line(here.0, here.1)),
            (true, Some(c)) => {
                path.push(PathOp::Quad(c.0, c.1, here.0, here.1));
                control = None;
            }
            (false, None) => control = Some(here),
            (false, Some(c)) => {
                let middle = ((c.0 + here.0) / 2.0, (c.1 + here.1) / 2.0);
                path.push(PathOp::Quad(c.0, c.1, middle.0, middle.1));
                control = Some(here);
            }
        }
    }
    if let Some(c) = control {
        path.push(PathOp::Quad(c.0, c.1, start.0, start.1));
    }
    path.push(PathOp::Close);
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A tiny hand-built TrueType font: glyph 1 is a square ('A'), glyph 2 a diamond made of
    /// off-curve points ('B'), glyph 3 a composite of glyph 1 moved right ('C'); 1000 units per em.
    pub(crate) fn tiny_font() -> Vec<u8> {
        fn be16(out: &mut Vec<u8>, value: i32) {
            out.extend_from_slice(&(value as u16).to_be_bytes());
        }
        fn simple(points: &[(i32, i32, bool)]) -> Vec<u8> {
            let mut glyph = Vec::new();
            be16(&mut glyph, 1);
            for value in [0, 0, 600, 700] {
                be16(&mut glyph, value);
            }
            be16(&mut glyph, points.len() as i32 - 1);
            be16(&mut glyph, 0);
            for &(_, _, on) in points {
                glyph.push(u8::from(on));
            }
            let (mut x, mut y) = (0, 0);
            let mut ys = Vec::new();
            for &(px, py, _) in points {
                be16(&mut glyph, px - x);
                be16(&mut ys, py - y);
                x = px;
                y = py;
            }
            glyph.extend_from_slice(&ys);
            while glyph.len() % 4 != 0 {
                glyph.push(0);
            }
            glyph
        }
        let square = simple(&[
            (100, 0, true),
            (100, 700, true),
            (500, 700, true),
            (500, 0, true),
        ]);
        let diamond = simple(&[
            (300, 0, false),
            (0, 350, false),
            (300, 700, false),
            (600, 350, false),
        ]);
        let mut composite = Vec::new();
        for value in [-1, 0, 0, 600, 700] {
            be16(&mut composite, value);
        }
        be16(&mut composite, 0x0001 | 0x0002); // words, xy values, last component
        be16(&mut composite, 1);
        be16(&mut composite, 300);
        be16(&mut composite, 0);
        let glyphs = [Vec::new(), square, diamond, composite];
        let mut glyf = Vec::new();
        let mut loca = Vec::new();
        for glyph in &glyphs {
            loca.extend_from_slice(&(glyf.len() as u32).to_be_bytes());
            glyf.extend_from_slice(glyph);
        }
        loca.extend_from_slice(&(glyf.len() as u32).to_be_bytes());
        let mut head = vec![0; 54];
        head[18..20].copy_from_slice(&1000_u16.to_be_bytes());
        head[50..52].copy_from_slice(&1_u16.to_be_bytes());
        let mut hhea = vec![0; 36];
        hhea[4..6].copy_from_slice(&800_i16.to_be_bytes());
        hhea[6..8].copy_from_slice(&(-200_i16).to_be_bytes());
        hhea[34..36].copy_from_slice(&4_u16.to_be_bytes());
        let mut maxp = vec![0; 6];
        maxp[4..6].copy_from_slice(&4_u16.to_be_bytes());
        let mut hmtx = Vec::new();
        for advance in [500, 600, 600, 900] {
            be16(&mut hmtx, advance);
            be16(&mut hmtx, 0);
        }
        // cmap format 4: 'A'..'C' -> 1..3, plus the end segment.
        let mut cmap = Vec::new();
        for value in [0, 1, 3, 1, 0, 12] {
            be16(&mut cmap, value);
        }
        let mut subtable = Vec::new();
        for value in [4, 32, 0, 4, 4, 1, 0] {
            be16(&mut subtable, value);
        }
        be16(&mut subtable, 0x43);
        be16(&mut subtable, 0xFFFF);
        be16(&mut subtable, 0);
        be16(&mut subtable, 0x41);
        be16(&mut subtable, 0xFFFF);
        be16(&mut subtable, 1 - 0x41);
        be16(&mut subtable, 1);
        be16(&mut subtable, 0);
        be16(&mut subtable, 0);
        cmap.extend_from_slice(&subtable);
        // kern: A then B pulls 100 units together.
        let mut kern = Vec::new();
        for value in [0, 1, 0, 20, 1, 1, 6, 0, 0] {
            be16(&mut kern, value);
        }
        be16(&mut kern, 1);
        be16(&mut kern, 2);
        be16(&mut kern, -100);
        let tables: [(&[u8; 4], Vec<u8>); 8] = [
            (b"cmap", cmap),
            (b"glyf", glyf),
            (b"head", head),
            (b"hhea", hhea),
            (b"hmtx", hmtx),
            (b"kern", kern),
            (b"loca", loca),
            (b"maxp", maxp),
        ];
        let mut font = Vec::new();
        font.extend_from_slice(&0x0001_0000_u32.to_be_bytes());
        be16(&mut font, tables.len() as i32);
        font.extend_from_slice(&[0; 6]);
        let mut offset = 12 + tables.len() * 16;
        let mut bodies = Vec::new();
        for (tag, body) in &tables {
            font.extend_from_slice(*tag);
            font.extend_from_slice(&[0; 4]);
            font.extend_from_slice(&(offset as u32).to_be_bytes());
            font.extend_from_slice(&(body.len() as u32).to_be_bytes());
            let mut padded = body.clone();
            while padded.len() % 4 != 0 {
                padded.push(0);
            }
            offset += padded.len();
            bodies.extend_from_slice(&padded);
        }
        font.extend_from_slice(&bodies);
        font
    }

    #[test]
    fn reads_maps_metrics_outlines_and_kerning() {
        let font = TrueType::parse(tiny_font(), 0).unwrap();
        assert_eq!(
            (font.glyph('A'), font.glyph('B'), font.glyph('C')),
            (1, 2, 3)
        );
        assert_eq!(font.glyph('z'), 0);
        assert_eq!(font.advance(3), 900.0);
        assert_eq!(font.kerning(1, 2), -100.0);
        assert_eq!(font.kerning(2, 1), 0.0);
        let square = font.outline(1);
        assert_eq!(square[0], PathOp::Move(100.0, 0.0));
        assert_eq!(square.len(), 6);
        // Four off-curve points: four curves through their midpoints.
        let diamond = font.outline(2);
        assert_eq!(
            diamond
                .iter()
                .filter(|op| matches!(op, PathOp::Quad(..)))
                .count(),
            4
        );
        // The composite is the square moved 300 units right.
        assert_eq!(font.outline(3)[0], PathOp::Move(400.0, 0.0));
        assert!(font.outline(0).is_empty());
    }

    #[test]
    fn damaged_fonts_are_errors() {
        assert_eq!(
            TrueType::parse(b"nope".to_vec(), 0).err(),
            Some(FontError::NotTrueType)
        );
        assert_eq!(
            TrueType::parse(b"OTTO\0\0\0\0".to_vec(), 0).err(),
            Some(FontError::CffOutlines)
        );
        let mut font = tiny_font();
        font.truncate(200);
        assert!(TrueType::parse(font, 0).is_err());
        // Garbage glyph data never panics.
        let mut font = tiny_font();
        let length = font.len();
        for byte in &mut font[length - 120..] {
            *byte = 0xFF;
        }
        if let Ok(font) = TrueType::parse(font, 0) {
            for glyph in 0..8 {
                let _ = font.outline(glyph);
            }
        }
    }
}
