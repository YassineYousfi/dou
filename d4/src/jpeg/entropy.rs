use std::io::{self, Read, Seek, SeekFrom, Write};

use super::{EOI, invalid_data};

// JPEG stores coefficients in this zig-zag order. The values are indices into
// a normal row-major 8x8 block.
pub(super) const ZIGZAG: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20,
    13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59,
    52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

pub(super) struct HuffmanTable {
    pub class: u8,
    pub id: u8,
    counts: [u8; 16],
    symbols: Vec<u8>,
    first_code: [u32; 16],
    first_symbol: [usize; 16],
    encode_code: [u16; 256],
    encode_length: [u8; 256],
}

impl HuffmanTable {
    pub fn new(class: u8, id: u8, counts: [u8; 16], symbols: Vec<u8>) -> io::Result<Self> {
        if counts
            .iter()
            .map(|&count| usize::from(count))
            .sum::<usize>()
            != symbols.len()
        {
            return Err(invalid_data("Huffman symbol count does not match table"));
        }

        let mut first_code = [0; 16];
        let mut first_symbol = [0; 16];
        let mut encode_code = [0; 256];
        let mut encode_length = [0; 256];
        let mut code = 0_u32;
        let mut symbol_index = 0;

        for length_index in 0..16 {
            first_code[length_index] = code;
            first_symbol[length_index] = symbol_index;
            let count = usize::from(counts[length_index]);

            if code + count as u32 > 1_u32 << (length_index + 1) {
                return Err(invalid_data("over-subscribed Huffman table"));
            }

            for &symbol in &symbols[symbol_index..symbol_index + count] {
                encode_code[usize::from(symbol)] = code as u16;
                encode_length[usize::from(symbol)] = (length_index + 1) as u8;
                code += 1;
            }

            symbol_index += count;
            code <<= 1;
        }

        Ok(Self {
            class,
            id,
            counts,
            symbols,
            first_code,
            first_symbol,
            encode_code,
            encode_length,
        })
    }

    pub fn decode<R: Read>(&self, bits: &mut BitReader<'_, R>) -> io::Result<u8> {
        let mut code = 0_u32;
        for length_index in 0..16 {
            code = (code << 1) | u32::from(bits.read_bit()?);
            let first = self.first_code[length_index];
            let count = u32::from(self.counts[length_index]);
            if code >= first && code < first + count {
                let index = self.first_symbol[length_index] + (code - first) as usize;
                return Ok(self.symbols[index]);
            }
        }
        Err(invalid_data("invalid Huffman code"))
    }

    pub fn encode<W: Write>(&self, symbol: u8, bits: &mut BitWriter<'_, W>) -> io::Result<()> {
        let length = self.encode_length[usize::from(symbol)];
        if length == 0 {
            return Err(invalid_data(format!(
                "Huffman table has no code for symbol 0x{symbol:02x}"
            )));
        }
        bits.write_bits(u32::from(self.encode_code[usize::from(symbol)]), length)
    }

    pub fn append_definition(&self, output: &mut Vec<u8>) {
        output.push(self.class << 4 | self.id);
        output.extend_from_slice(&self.counts);
        output.extend_from_slice(&self.symbols);
    }
}

pub(super) struct BitReader<'a, R> {
    reader: &'a mut R,
    current: u8,
    remaining: u8,
}

// The low 12 bits hold the current entropy byte and its remaining bit count;
// the upper bits hold the raw file position of the next unread byte. Packing
// these together keeps the per-MCU seek index to eight bytes per entry.
#[derive(Clone, Copy)]
pub(super) struct BitReaderState(u64);

impl<'a, R: Read> BitReader<'a, R> {
    pub fn new(reader: &'a mut R) -> Self {
        Self {
            reader,
            current: 0,
            remaining: 0,
        }
    }

    pub fn read_bit(&mut self) -> io::Result<u8> {
        if self.remaining == 0 {
            self.current = self.read_entropy_byte()?;
            self.remaining = 8;
        }
        self.remaining -= 1;
        Ok(self.current >> self.remaining & 1)
    }

    pub fn read_bits(&mut self, count: u8) -> io::Result<u32> {
        let mut value = 0;
        for _ in 0..count {
            value = value << 1 | u32::from(self.read_bit()?);
        }
        Ok(value)
    }

    fn read_entropy_byte(&mut self) -> io::Result<u8> {
        let byte = read_u8(self.reader)?;
        if byte != 0xff {
            return Ok(byte);
        }

        let next = read_u8(self.reader)?;
        if next == 0x00 {
            Ok(0xff)
        } else {
            Err(invalid_data(format!(
                "unexpected marker ff{next:02x} inside entropy data"
            )))
        }
    }

    pub fn finish_scan(&mut self) -> io::Result<()> {
        self.remaining = 0;
        if read_u8(self.reader)? != 0xff {
            return Err(invalid_data("entropy scan is not followed by a marker"));
        }

        let mut marker = read_u8(self.reader)?;
        while marker == 0xff {
            marker = read_u8(self.reader)?;
        }
        if marker != EOI {
            return Err(invalid_data(format!(
                "expected EOI after entropy scan, found ff{marker:02x}"
            )));
        }
        Ok(())
    }
}

impl<'a, R: Read + Seek> BitReader<'a, R> {
    pub fn state(&mut self) -> io::Result<BitReaderState> {
        let position = self.reader.stream_position()?;
        if position > u64::MAX >> 12 {
            return Err(invalid_data("JPEG file position is too large"));
        }
        Ok(BitReaderState(
            position << 12 | u64::from(self.current) << 4 | u64::from(self.remaining),
        ))
    }

    pub fn from_state(reader: &'a mut R, state: BitReaderState) -> io::Result<Self> {
        reader.seek(SeekFrom::Start(state.0 >> 12))?;
        Ok(Self {
            reader,
            current: (state.0 >> 4) as u8,
            remaining: (state.0 & 0x0f) as u8,
        })
    }
}

pub(super) struct BitWriter<'a, W> {
    writer: &'a mut W,
    current: u8,
    used: u8,
}

impl<'a, W: Write> BitWriter<'a, W> {
    pub fn new(writer: &'a mut W) -> Self {
        Self {
            writer,
            current: 0,
            used: 0,
        }
    }

    pub fn write_bits(&mut self, value: u32, count: u8) -> io::Result<()> {
        for shift in (0..count).rev() {
            self.current = self.current << 1 | ((value >> shift) as u8 & 1);
            self.used += 1;
            if self.used == 8 {
                self.write_entropy_byte(self.current)?;
                self.current = 0;
                self.used = 0;
            }
        }
        Ok(())
    }

    pub fn finish(&mut self) -> io::Result<()> {
        if self.used != 0 {
            let padding = 8 - self.used;
            self.current = self.current << padding | ((1_u16 << padding) - 1) as u8;
            self.write_entropy_byte(self.current)?;
            self.current = 0;
            self.used = 0;
        }
        Ok(())
    }

    fn write_entropy_byte(&mut self, byte: u8) -> io::Result<()> {
        self.writer.write_all(&[byte])?;
        if byte == 0xff {
            self.writer.write_all(&[0x00])?;
        }
        Ok(())
    }
}

fn read_u8<R: Read>(reader: &mut R) -> io::Result<u8> {
    let mut byte = [0];
    reader.read_exact(&mut byte)?;
    Ok(byte[0])
}
