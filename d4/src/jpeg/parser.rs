use std::io::{self, Read, Seek};

use super::entropy::{BitReader, BitReaderState, HuffmanTable, ZIGZAG};
use super::{DHT, DRI, EOI, SOF0, SOI, SOS, invalid_data};

pub(super) struct Segment {
    pub marker: u8,
    pub data: Vec<u8>,
}

pub(super) struct Component {
    pub id: u8,
    pub horizontal_sampling: usize,
    pub vertical_sampling: usize,
}

pub(super) struct Frame {
    pub width: usize,
    pub height: usize,
    pub max_horizontal_sampling: usize,
    pub max_vertical_sampling: usize,
    pub components: Vec<Component>,
}

pub(super) struct ScanComponent {
    pub frame_index: usize,
    pub dc_table: u8,
    pub ac_table: u8,
    pub first_block: usize,
}

pub(super) struct Scan {
    pub components: Vec<ScanComponent>,
    pub blocks_per_mcu: usize,
}

pub(super) struct Header {
    pub segments: Vec<Segment>,
    pub frame: Frame,
    pub scan: Scan,
    huffman_tables: Vec<HuffmanTable>,
}

pub(super) struct McuIndex {
    bit_states: Vec<BitReaderState>,
    dc_predictors: Vec<i16>,
}

impl McuIndex {
    pub fn storage_bytes(&self) -> usize {
        self.bit_states.capacity() * size_of::<BitReaderState>()
            + self.dc_predictors.capacity() * size_of::<i16>()
    }
}

impl Header {
    pub fn mcu_columns(&self) -> usize {
        self.frame
            .width
            .div_ceil(8 * self.frame.max_horizontal_sampling)
    }

    pub fn mcu_rows(&self) -> usize {
        self.frame
            .height
            .div_ceil(8 * self.frame.max_vertical_sampling)
    }

    fn huffman_table(&self, class: u8, id: u8) -> io::Result<&HuffmanTable> {
        self.huffman_tables
            .iter()
            .find(|table| table.class == class && table.id == id)
            .ok_or_else(|| invalid_data(format!("missing Huffman table class {class}, id {id}")))
    }
}

pub(super) fn parse_header<R: Read>(reader: &mut R) -> io::Result<Header> {
    if read_u8(reader)? != 0xff || read_u8(reader)? != SOI {
        return Err(invalid_data("not a JPEG file"));
    }

    let mut segments = Vec::new();
    let mut frame = None;
    let mut huffman_tables = Vec::new();
    let mut restart_interval = 0_u16;

    loop {
        let marker = read_marker(reader)?;
        if marker == EOI {
            return Err(invalid_data("JPEG ended before its scan"));
        }
        if marker == SOI || marker == 0x01 || (0xd0..=0xd7).contains(&marker) {
            return Err(invalid_data(format!(
                "unexpected standalone marker ff{marker:02x} in header"
            )));
        }

        let length = usize::from(read_u16(reader)?);
        if length < 2 {
            return Err(invalid_data("invalid JPEG segment length"));
        }
        let mut data = vec![0; length - 2];
        reader.read_exact(&mut data)?;

        match marker {
            SOF0 => frame = Some(parse_frame(&data)?),
            DHT => parse_huffman_tables(&data, &mut huffman_tables)?,
            DRI => {
                if data.len() != 2 {
                    return Err(invalid_data("invalid DRI segment"));
                }
                restart_interval = u16::from_be_bytes([data[0], data[1]]);
            }
            0xc1..=0xcf if marker != DHT => {
                return Err(invalid_data(
                    "only baseline sequential (SOF0) JPEG is supported",
                ));
            }
            _ => {}
        }

        // Input Huffman definitions have already been parsed. The writer emits
        // its own tables, so retaining the original DHT payload serves no use.
        if marker != DHT {
            segments.push(Segment { marker, data });
        }
        if marker == SOS {
            break;
        }
    }

    if restart_interval != 0 {
        return Err(invalid_data("restart markers are not used by this project"));
    }
    let frame = frame.ok_or_else(|| invalid_data("missing SOF0 frame"))?;
    let scan_data = &segments
        .last()
        .ok_or_else(|| invalid_data("missing SOS segment"))?
        .data;
    let scan = parse_scan(scan_data, &frame)?;

    Ok(Header {
        segments,
        frame,
        scan,
        huffman_tables,
    })
}

pub(super) fn validate_supported_layout(header: &Header) -> io::Result<()> {
    if header.scan.components.len() != header.frame.components.len() {
        return Err(invalid_data(
            "expected one interleaved scan of all components",
        ));
    }
    for scan_component in &header.scan.components {
        header.huffman_table(0, scan_component.dc_table)?;
        header.huffman_table(1, scan_component.ac_table)?;
    }
    Ok(())
}

pub(super) fn index_mcus<R: Read + Seek>(header: &Header, input: &mut R) -> io::Result<McuIndex> {
    let mut bits = BitReader::new(input);
    let mut dc_predictors = vec![0_i32; header.frame.components.len()];
    let mcu_count = header.mcu_columns() * header.mcu_rows();
    let mut index = McuIndex {
        bit_states: Vec::with_capacity(mcu_count),
        dc_predictors: Vec::with_capacity(mcu_count * header.frame.components.len()),
    };

    for _ in 0..mcu_count {
        index.bit_states.push(bits.state()?);
        index
            .dc_predictors
            .extend(dc_predictors.iter().map(|&predictor| predictor as i16));

        for scan_component in &header.scan.components {
            let component = &header.frame.components[scan_component.frame_index];
            let dc_table = header.huffman_table(0, scan_component.dc_table)?;
            let ac_table = header.huffman_table(1, scan_component.ac_table)?;
            for _ in 0..component.horizontal_sampling * component.vertical_sampling {
                decode_block(
                    &mut bits,
                    dc_table,
                    ac_table,
                    &mut dc_predictors[scan_component.frame_index],
                )?;
            }
        }
    }
    bits.finish_scan()?;
    Ok(index)
}

pub(super) fn decode_indexed_mcu<R: Read + Seek>(
    header: &Header,
    input: &mut R,
    index: &McuIndex,
    mcu: usize,
    blocks: &mut [i16],
    dc_predictors: &mut [i32],
) -> io::Result<()> {
    let predictor_start = mcu * header.frame.components.len();
    for (destination, &source) in dc_predictors
        .iter_mut()
        .zip(&index.dc_predictors[predictor_start..predictor_start + header.frame.components.len()])
    {
        *destination = i32::from(source);
    }

    let mut bits = BitReader::from_state(input, index.bit_states[mcu])?;
    let mut block_index = 0;
    for scan_component in &header.scan.components {
        let component = &header.frame.components[scan_component.frame_index];
        let dc_table = header.huffman_table(0, scan_component.dc_table)?;
        let ac_table = header.huffman_table(1, scan_component.ac_table)?;
        for _ in 0..component.horizontal_sampling * component.vertical_sampling {
            let block = decode_block(
                &mut bits,
                dc_table,
                ac_table,
                &mut dc_predictors[scan_component.frame_index],
            )?;
            blocks[block_index * 64..(block_index + 1) * 64].copy_from_slice(&block);
            block_index += 1;
        }
    }
    Ok(())
}

fn parse_frame(data: &[u8]) -> io::Result<Frame> {
    if data.len() < 6 || data[0] != 8 {
        return Err(invalid_data("expected an 8-bit SOF0 frame"));
    }
    let component_count = usize::from(data[5]);
    if data.len() != 6 + 3 * component_count {
        return Err(invalid_data("invalid SOF0 component list"));
    }

    let height = usize::from(u16::from_be_bytes([data[1], data[2]]));
    let width = usize::from(u16::from_be_bytes([data[3], data[4]]));
    let mut components = Vec::with_capacity(component_count);

    for index in 0..component_count {
        let offset = 6 + 3 * index;
        let sampling = data[offset + 1];
        let horizontal_sampling = usize::from(sampling >> 4);
        let vertical_sampling = usize::from(sampling & 0x0f);
        if horizontal_sampling == 0 || vertical_sampling == 0 {
            return Err(invalid_data("zero JPEG sampling factor"));
        }
        components.push(Component {
            id: data[offset],
            horizontal_sampling,
            vertical_sampling,
        });
    }

    let max_horizontal_sampling = components
        .iter()
        .map(|component| component.horizontal_sampling)
        .max()
        .ok_or_else(|| invalid_data("JPEG frame has no components"))?;
    let max_vertical_sampling = components
        .iter()
        .map(|component| component.vertical_sampling)
        .max()
        .ok_or_else(|| invalid_data("JPEG frame has no components"))?;

    Ok(Frame {
        width,
        height,
        max_horizontal_sampling,
        max_vertical_sampling,
        components,
    })
}

fn parse_scan(data: &[u8], frame: &Frame) -> io::Result<Scan> {
    if data.is_empty() {
        return Err(invalid_data("empty SOS segment"));
    }
    let component_count = usize::from(data[0]);
    if data.len() != 1 + 2 * component_count + 3 {
        return Err(invalid_data("invalid SOS component list"));
    }
    let tail = 1 + 2 * component_count;
    if data[tail] != 0 || data[tail + 1] != 63 || data[tail + 2] != 0 {
        return Err(invalid_data("expected a baseline sequential scan"));
    }

    let mut components = Vec::with_capacity(component_count);
    let mut blocks_per_mcu = 0;
    for index in 0..component_count {
        let selector = data[1 + 2 * index];
        let tables = data[2 + 2 * index];
        let frame_index = frame
            .components
            .iter()
            .position(|component| component.id == selector)
            .ok_or_else(|| invalid_data("SOS refers to an unknown component"))?;
        components.push(ScanComponent {
            frame_index,
            dc_table: tables >> 4,
            ac_table: tables & 0x0f,
            first_block: blocks_per_mcu,
        });
        let component = &frame.components[frame_index];
        blocks_per_mcu += component.horizontal_sampling * component.vertical_sampling;
    }

    Ok(Scan {
        components,
        blocks_per_mcu,
    })
}

fn parse_huffman_tables(data: &[u8], tables: &mut Vec<HuffmanTable>) -> io::Result<()> {
    let mut position = 0;
    while position < data.len() {
        if data.len() - position < 17 {
            return Err(invalid_data("truncated DHT table"));
        }
        let definition = data[position];
        position += 1;
        let class = definition >> 4;
        let id = definition & 0x0f;
        if class > 1 || id > 3 {
            return Err(invalid_data("invalid Huffman table selector"));
        }

        let mut counts = [0; 16];
        counts.copy_from_slice(&data[position..position + 16]);
        position += 16;
        let symbol_count = counts
            .iter()
            .map(|&count| usize::from(count))
            .sum::<usize>();
        if data.len() - position < symbol_count {
            return Err(invalid_data("truncated DHT symbols"));
        }
        let symbols = data[position..position + symbol_count].to_vec();
        position += symbol_count;

        tables.retain(|table| table.class != class || table.id != id);
        tables.push(HuffmanTable::new(class, id, counts, symbols)?);
    }
    Ok(())
}

fn decode_block<R: Read>(
    bits: &mut BitReader<'_, R>,
    dc_table: &HuffmanTable,
    ac_table: &HuffmanTable,
    dc_predictor: &mut i32,
) -> io::Result<[i16; 64]> {
    let mut block = [0_i16; 64];
    let dc_size = dc_table.decode(bits)?;
    if dc_size > 11 {
        return Err(invalid_data("invalid baseline DC magnitude"));
    }
    *dc_predictor += receive_extend(bits, dc_size)?;
    block[0] = i16::try_from(*dc_predictor)
        .map_err(|_| invalid_data("DC coefficient is outside i16 range"))?;

    let mut zigzag_index = 1;
    while zigzag_index < 64 {
        let symbol = ac_table.decode(bits)?;
        let run = usize::from(symbol >> 4);
        let size = symbol & 0x0f;

        if size == 0 {
            if run == 0 {
                break;
            }
            if run != 15 {
                return Err(invalid_data("invalid zero-run AC symbol"));
            }
            zigzag_index += 16;
            if zigzag_index > 64 {
                return Err(invalid_data("AC zero run exceeds block"));
            }
            continue;
        }

        zigzag_index += run;
        if zigzag_index >= 64 || size > 10 {
            return Err(invalid_data("invalid baseline AC coefficient"));
        }
        block[ZIGZAG[zigzag_index]] = i16::try_from(receive_extend(bits, size)?)
            .map_err(|_| invalid_data("AC coefficient is outside i16 range"))?;
        zigzag_index += 1;
    }
    Ok(block)
}

fn receive_extend<R: Read>(bits: &mut BitReader<'_, R>, size: u8) -> io::Result<i32> {
    if size == 0 {
        return Ok(0);
    }
    let value = bits.read_bits(size)? as i32;
    let threshold = 1_i32 << (size - 1);
    if value < threshold {
        Ok(value + 1 - (1_i32 << size))
    } else {
        Ok(value)
    }
}

fn read_marker<R: Read>(reader: &mut R) -> io::Result<u8> {
    loop {
        if read_u8(reader)? != 0xff {
            continue;
        }
        let mut marker = read_u8(reader)?;
        while marker == 0xff {
            marker = read_u8(reader)?;
        }
        if marker != 0x00 {
            return Ok(marker);
        }
    }
}

fn read_u8<R: Read>(reader: &mut R) -> io::Result<u8> {
    let mut byte = [0];
    reader.read_exact(&mut byte)?;
    Ok(byte[0])
}

fn read_u16<R: Read>(reader: &mut R) -> io::Result<u16> {
    let mut bytes = [0; 2];
    reader.read_exact(&mut bytes)?;
    Ok(u16::from_be_bytes(bytes))
}
