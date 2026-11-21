//! A minimal terminal screen model for checking presenter output by its
//! effect rather than its exact bytes. It understands only what the
//! presenter emits: cursor positioning, erase display, SGR colors, and the
//! synchronized-update private mode.

/// A pen color as the terminal understands it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Pen {
    Default,
    Rgb(u8, u8, u8),
    Indexed256(u8),
    Indexed16(u8),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScreenCell {
    pub glyph: char,
    pub foreground: Pen,
    pub background: Pen,
    /// SGR attributes in `trend::Cell` bit order (bold 1, dim 2, italic 4,
    /// underline 8, blink 16, reverse 32, strikethrough 64).
    pub attributes: u8,
}

impl ScreenCell {
    const BLANK: Self = Self {
        glyph: ' ',
        foreground: Pen::Default,
        background: Pen::Default,
        attributes: 0,
    };
}

#[derive(Clone, Debug)]
pub struct Screen {
    pub width: usize,
    pub height: usize,
    pub cells: Vec<ScreenCell>,
    row: usize,
    column: usize,
    foreground: Pen,
    background: Pen,
    attributes: u8,
    /// Nesting state of DEC mode 2026; must be closed after every frame.
    pub synchronized: bool,
}

/// Screens are equal when they show the same thing; the cursor and pen
/// don't matter.
impl PartialEq for Screen {
    fn eq(&self, other: &Self) -> bool {
        self.width == other.width && self.height == other.height && self.cells == other.cells
    }
}

impl Eq for Screen {}

impl Screen {
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            cells: vec![ScreenCell::BLANK; width * height],
            row: 1,
            column: 1,
            foreground: Pen::Default,
            background: Pen::Default,
            attributes: 0,
            synchronized: false,
        }
    }

    /// The glyphs of one 1-based row, for readable assertions.
    pub fn text(&self, row: usize) -> String {
        self.cells[(row - 1) * self.width..row * self.width]
            .iter()
            .map(|cell| cell.glyph)
            .collect()
    }

    pub fn at(&self, row: usize, column: usize) -> ScreenCell {
        self.cells[(row - 1) * self.width + column - 1]
    }

    /// The current pen, to check that positioned output restores defaults.
    pub fn pen(&self) -> (Pen, Pen) {
        (self.foreground, self.background)
    }

    /// The current attributes, to check that positioned output resets them.
    pub fn pen_attributes(&self) -> u8 {
        self.attributes
    }

    pub fn apply(&mut self, bytes: &[u8]) {
        let text = std::str::from_utf8(bytes).expect("presenter output is UTF-8");
        let mut chars = text.chars().peekable();
        while let Some(ch) = chars.next() {
            if ch != '\x1b' {
                self.put(ch);
                continue;
            }
            assert_eq!(chars.next(), Some('['), "only CSI sequences are emitted");
            let private = chars.peek() == Some(&'?');
            if private {
                chars.next();
            }
            let mut parameters = String::new();
            let last = loop {
                let next = chars.next().expect("unterminated escape sequence");
                if next.is_ascii_digit() || next == ';' {
                    parameters.push(next);
                } else {
                    break next;
                }
            };
            let numbers: Vec<u16> = parameters
                .split(';')
                .map(|part| {
                    if part.is_empty() {
                        0
                    } else {
                        part.parse().unwrap()
                    }
                })
                .collect();
            match (private, last) {
                (true, 'h') => {
                    assert_eq!(numbers, [2026]);
                    assert!(!self.synchronized, "nested synchronized update");
                    self.synchronized = true;
                }
                (true, 'l') => {
                    assert_eq!(numbers, [2026]);
                    assert!(self.synchronized, "unbalanced synchronized update");
                    self.synchronized = false;
                }
                (false, 'H') => {
                    self.row = usize::from(numbers[0]);
                    self.column = usize::from(numbers[1]);
                }
                (false, 'J') => {
                    assert_eq!(numbers, [2]);
                    self.cells.fill(ScreenCell::BLANK);
                }
                (false, 'm') => self.sgr(&numbers),
                other => panic!("unexpected sequence {other:?} {parameters:?}"),
            }
        }
    }

    fn put(&mut self, glyph: char) {
        if (1..=self.height).contains(&self.row) && (1..=self.width).contains(&self.column) {
            self.cells[(self.row - 1) * self.width + self.column - 1] = ScreenCell {
                glyph,
                foreground: self.foreground,
                background: self.background,
                attributes: self.attributes,
            };
        }
        self.column += 1;
    }

    fn sgr(&mut self, numbers: &[u16]) {
        let mut index = 0;
        while index < numbers.len() {
            let code = numbers[index];
            index += 1;
            match code {
                0 => {
                    self.foreground = Pen::Default;
                    self.background = Pen::Default;
                    self.attributes = 0;
                }
                1 => self.attributes |= 1,
                2 => self.attributes |= 2,
                3 => self.attributes |= 4,
                4 => self.attributes |= 8,
                5 => self.attributes |= 16,
                7 => self.attributes |= 32,
                9 => self.attributes |= 64,
                22 => self.attributes &= !3,
                23 => self.attributes &= !4,
                24 => self.attributes &= !8,
                25 => self.attributes &= !16,
                27 => self.attributes &= !32,
                29 => self.attributes &= !64,
                39 => self.foreground = Pen::Default,
                49 => self.background = Pen::Default,
                38 | 48 => {
                    let pen = match numbers[index] {
                        2 => {
                            index += 4;
                            Pen::Rgb(
                                numbers[index - 3] as u8,
                                numbers[index - 2] as u8,
                                numbers[index - 1] as u8,
                            )
                        }
                        5 => {
                            index += 2;
                            Pen::Indexed256(numbers[index - 1] as u8)
                        }
                        other => panic!("unexpected extended color {other}"),
                    };
                    if code == 38 {
                        self.foreground = pen;
                    } else {
                        self.background = pen;
                    }
                }
                30..=37 => self.foreground = Pen::Indexed16((code - 30) as u8),
                90..=97 => self.foreground = Pen::Indexed16((code - 90 + 8) as u8),
                40..=47 => self.background = Pen::Indexed16((code - 40) as u8),
                100..=107 => self.background = Pen::Indexed16((code - 100 + 8) as u8),
                other => panic!("unexpected SGR {other}"),
            }
        }
    }
}
