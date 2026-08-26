use std::fs::File;
use std::io::{self, BufWriter, Read, Seek, Write};
use std::path::Path;

use super::entropy::{BitWriter, HuffmanTable, ZIGZAG};
use super::parser::{Header, McuIndex, decode_indexed_mcu};
use super::transform::{Transform, transpose_frequency_table};
use super::{DHT, DQT, EOI, IO_BUFFER_SIZE, SOF0, SOI, SOS, invalid_data};

fn output_huffman_tables() -> io::Result<[HuffmanTable; 2]> {
    let mut dc_counts = [0; 16];
    dc_counts[3] = 12;
    let dc_symbols = (0..=11).collect();

    let mut ac_counts = [0; 16];
    ac_counts[7] = 162;
    let mut ac_symbols = Vec::with_capacity(162);
    ac_symbols.extend_from_slice(&[0x00, 0xf0]);
    for run in 0..16 {
        for size in 1..=10 {
            ac_symbols.push((run << 4) | size);
        }
    }

    Ok([
        HuffmanTable::new(0, 0, dc_counts, dc_symbols)?,
        HuffmanTable::new(1, 0, ac_counts, ac_symbols)?,
    ])
}

pub(super) fn write_transformed<R: Read + Seek>(
    header: &Header,
    input: &mut R,
    index: &McuIndex,
    output_path: &Path,
    transform: Transform,
) -> io::Result<()> {
    let output_tables = output_huffman_tables()?;
    let output = File::create(output_path)?;
    let mut output = BufWriter::with_capacity(IO_BUFFER_SIZE, output);
    output.write_all(&[0xff, SOI])?;
    write_headers(&mut output, header, transform, &output_tables)?;
    write_coefficients(&mut output, header, input, index, transform, &output_tables)?;
    output.write_all(&[0xff, EOI])?;
    output.flush()
}

fn write_headers<W: Write>(
    output: &mut W,
    header: &Header,
    transform: Transform,
    output_tables: &[HuffmanTable],
) -> io::Result<()> {
    for segment in &header.segments {
        match segment.marker {
            SOS => {
                write_huffman_segment(output, output_tables)?;
                let mut data = segment.data.clone();
                let count = usize::from(data[0]);
                for index in 0..count {
                    data[2 + 2 * index] = 0;
                }
                write_segment(output, SOS, &data)?;
            }
            SOF0 => {
                let data = rewrite_frame_segment(&segment.data, transform)?;
                write_segment(output, SOF0, &data)?;
            }
            DQT if transform.swaps_axes() => {
                let data = rewrite_quantization_segment(&segment.data)?;
                write_segment(output, DQT, &data)?;
            }
            _ => write_segment(output, segment.marker, &segment.data)?,
        }
    }
    Ok(())
}

fn write_coefficients<R: Read + Seek, W: Write>(
    output: &mut W,
    header: &Header,
    input: &mut R,
    index: &McuIndex,
    transform: Transform,
    output_tables: &[HuffmanTable],
) -> io::Result<()> {
    let source_mcu_columns = header.mcu_columns();
    let source_mcu_rows = header.mcu_rows();
    let (output_mcu_columns, output_mcu_rows) = if transform.swaps_axes() {
        (source_mcu_rows, source_mcu_columns)
    } else {
        (source_mcu_columns, source_mcu_rows)
    };
    let mut source_blocks = vec![0_i16; header.scan.blocks_per_mcu * 64];
    let mut source_dc_predictors = vec![0_i32; header.frame.components.len()];
    let mut output_dc_predictors = vec![0_i32; header.frame.components.len()];
    let mut bits = BitWriter::new(output);

    for destination_y in 0..output_mcu_rows {
        for destination_x in 0..output_mcu_columns {
            let (source_x, source_y) = transform.source_position(
                destination_x,
                destination_y,
                source_mcu_columns,
                source_mcu_rows,
            );
            let source_mcu = source_y * source_mcu_columns + source_x;
            decode_indexed_mcu(
                header,
                input,
                index,
                source_mcu,
                &mut source_blocks,
                &mut source_dc_predictors,
            )?;

            for scan_component in &header.scan.components {
                let component = &header.frame.components[scan_component.frame_index];
                let (output_horizontal, output_vertical) = if transform.swaps_axes() {
                    (component.vertical_sampling, component.horizontal_sampling)
                } else {
                    (component.horizontal_sampling, component.vertical_sampling)
                };
                for output_v in 0..output_vertical {
                    for output_h in 0..output_horizontal {
                        let (source_h, source_v) = transform.source_position(
                            output_h,
                            output_v,
                            component.horizontal_sampling,
                            component.vertical_sampling,
                        );
                        let source_block = scan_component.first_block
                            + source_v * component.horizontal_sampling
                            + source_h;
                        let block_start = source_block * 64;
                        let block = transform.apply_block(|source_u, source_v| {
                            source_blocks[block_start + source_v * 8 + source_u]
                        });
                        encode_block(
                            &mut bits,
                            &output_tables[0],
                            &output_tables[1],
                            &mut output_dc_predictors[scan_component.frame_index],
                            &block,
                        )?;
                    }
                }
            }
        }
    }
    bits.finish()
}

fn encode_block<W: Write>(
    bits: &mut BitWriter<'_, W>,
    dc_table: &HuffmanTable,
    ac_table: &HuffmanTable,
    dc_predictor: &mut i32,
    block: &[i16; 64],
) -> io::Result<()> {
    let dc = i32::from(block[0]);
    let difference = dc - *dc_predictor;
    *dc_predictor = dc;
    let dc_size = magnitude_size(difference);
    if dc_size > 11 {
        return Err(invalid_data(
            "transformed DC difference exceeds baseline range",
        ));
    }
    dc_table.encode(dc_size, bits)?;
    write_magnitude(bits, difference, dc_size)?;

    let mut zero_run = 0;
    for &natural_index in &ZIGZAG[1..] {
        let coefficient = i32::from(block[natural_index]);
        if coefficient == 0 {
            zero_run += 1;
            continue;
        }

        while zero_run >= 16 {
            ac_table.encode(0xf0, bits)?;
            zero_run -= 16;
        }
        let size = magnitude_size(coefficient);
        if size > 10 {
            return Err(invalid_data(
                "transformed AC coefficient exceeds baseline range",
            ));
        }
        ac_table.encode((zero_run << 4) as u8 | size, bits)?;
        write_magnitude(bits, coefficient, size)?;
        zero_run = 0;
    }

    if zero_run != 0 {
        ac_table.encode(0x00, bits)?;
    }
    Ok(())
}

fn magnitude_size(value: i32) -> u8 {
    if value == 0 {
        0
    } else {
        (32 - value.unsigned_abs().leading_zeros()) as u8
    }
}

fn write_magnitude<W: Write>(bits: &mut BitWriter<'_, W>, value: i32, size: u8) -> io::Result<()> {
    if size == 0 {
        return Ok(());
    }
    let encoded = if value < 0 {
        value + (1_i32 << size) - 1
    } else {
        value
    };
    bits.write_bits(encoded as u32, size)
}

fn write_huffman_segment<W: Write>(output: &mut W, tables: &[HuffmanTable]) -> io::Result<()> {
    let mut data = Vec::new();
    for table in tables {
        table.append_definition(&mut data);
    }
    write_segment(output, DHT, &data)
}

fn rewrite_frame_segment(data: &[u8], transform: Transform) -> io::Result<Vec<u8>> {
    if data.len() < 6 {
        return Err(invalid_data("truncated SOF0 segment"));
    }
    let mut output = data.to_vec();
    if transform.swaps_axes() {
        let height = [data[1], data[2]];
        let width = [data[3], data[4]];
        output[1..3].copy_from_slice(&width);
        output[3..5].copy_from_slice(&height);

        let component_count = usize::from(data[5]);
        for index in 0..component_count {
            let sampling_index = 7 + 3 * index;
            output[sampling_index] = data[sampling_index].rotate_right(4);
        }
    }
    Ok(output)
}

fn rewrite_quantization_segment(data: &[u8]) -> io::Result<Vec<u8>> {
    let mut output = data.to_vec();
    let mut position = 0;
    while position < data.len() {
        let precision = data[position] >> 4;
        if precision > 1 {
            return Err(invalid_data("invalid DQT precision"));
        }
        position += 1;
        let value_bytes = usize::from(precision) + 1;
        let table_bytes = 64 * value_bytes;
        if data.len() - position < table_bytes {
            return Err(invalid_data("truncated DQT table"));
        }

        let mut natural = [0_u16; 64];
        for (zigzag_index, &natural_index) in ZIGZAG.iter().enumerate() {
            let offset = position + zigzag_index * value_bytes;
            natural[natural_index] = if value_bytes == 1 {
                u16::from(data[offset])
            } else {
                u16::from_be_bytes([data[offset], data[offset + 1]])
            };
        }
        let transposed = transpose_frequency_table(&natural);

        for (zigzag_index, &natural_index) in ZIGZAG.iter().enumerate() {
            let value = transposed[natural_index];
            let offset = position + zigzag_index * value_bytes;
            if value_bytes == 1 {
                output[offset] = value as u8;
            } else {
                output[offset..offset + 2].copy_from_slice(&value.to_be_bytes());
            }
        }
        position += table_bytes;
    }
    Ok(output)
}

fn write_segment<W: Write>(output: &mut W, marker: u8, data: &[u8]) -> io::Result<()> {
    let length =
        u16::try_from(data.len() + 2).map_err(|_| invalid_data("JPEG segment is too large"))?;
    output.write_all(&[0xff, marker])?;
    output.write_all(&length.to_be_bytes())?;
    output.write_all(data)
}
