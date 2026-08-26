mod entropy;
mod parser;
mod transform;
mod writer;

use std::fs::{self, File};
use std::io::{self, BufReader};
use std::path::Path;

use parser::{index_mcus, parse_header, validate_supported_layout};
use transform::Transform;
use writer::write_transformed;

pub(super) const SOI: u8 = 0xd8;
pub(super) const EOI: u8 = 0xd9;
pub(super) const DHT: u8 = 0xc4;
pub(super) const DQT: u8 = 0xdb;
pub(super) const DRI: u8 = 0xdd;
pub(super) const SOF0: u8 = 0xc0;
pub(super) const SOS: u8 = 0xda;
pub(super) const IO_BUFFER_SIZE: usize = 8 * 1024;

pub struct Report {
    pub index_storage_bytes: usize,
    pub tracked_working_bytes: usize,
}

pub fn transform(input_path: &Path, output_path: &Path, operation: u8) -> io::Result<Report> {
    let transform = Transform::from_index(operation).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "operation must be an integer from 0 through 7",
        )
    })?;
    let input = File::open(input_path)?;
    let mut input = BufReader::with_capacity(IO_BUFFER_SIZE, input);
    let header = parse_header(&mut input)?;
    validate_supported_layout(&header)?;
    if !header
        .frame
        .width
        .is_multiple_of(8 * header.frame.max_horizontal_sampling)
        || !header
            .frame
            .height
            .is_multiple_of(8 * header.frame.max_vertical_sampling)
    {
        return Err(invalid_data(
            "lossless transforms require image dimensions on MCU boundaries",
        ));
    }

    if let Some(parent) = output_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    let index = index_mcus(&header, &mut input)?;

    write_transformed(&header, &mut input, &index, output_path, transform)?;

    let index_storage_bytes = index.storage_bytes();
    let source_mcu_bytes = header.scan.blocks_per_mcu * 64 * size_of::<i16>();
    let transform_block_bytes = 64 * size_of::<i16>();
    let io_buffer_bytes = 2 * IO_BUFFER_SIZE;
    let predictor_scratch_bytes = 2 * header.frame.components.len() * size_of::<i32>();

    Ok(Report {
        index_storage_bytes,
        tracked_working_bytes: index_storage_bytes
            + source_mcu_bytes
            + transform_block_bytes
            + io_buffer_bytes
            + predictor_scratch_bytes,
    })
}

pub fn operation_filename(operation: u8) -> Option<String> {
    Transform::from_index(operation).map(Transform::filename)
}

pub(super) fn invalid_data(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}
