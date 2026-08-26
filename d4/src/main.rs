mod jpeg;

use std::env;
use std::io;
use std::path::PathBuf;

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

fn run() -> io::Result<()> {
    let mut args = env::args_os();
    let program = args.next().unwrap_or_default();
    let Some(input) = args.next() else {
        print_usage(&PathBuf::from(program));
        return Ok(());
    };
    let Some(operation) = args.next() else {
        print_usage(&PathBuf::from(program));
        return Ok(());
    };

    let input = PathBuf::from(input);
    let operation_text = operation.to_string_lossy();
    let operation = operation_text.parse::<u8>().map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "operation must be an integer from 0 through 7",
        )
    })?;
    let filename = jpeg::operation_filename(operation).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "operation must be an integer from 0 through 7",
        )
    })?;
    let output_path = args.next().map(PathBuf::from).unwrap_or_else(|| {
        input
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."))
            .join("augmented")
            .join(filename)
    });

    if args.next().is_some() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected at most three arguments",
        ));
    }

    let report = jpeg::transform(&input, &output_path, operation)?;

    println!("wrote operation {} to {}", operation, output_path.display());
    println!(
        "working payload: {} bytes ({:.2} MiB), including a {}-byte MCU index",
        report.tracked_working_bytes,
        report.tracked_working_bytes as f64 / (1024.0 * 1024.0),
        report.index_storage_bytes,
    );

    Ok(())
}

fn print_usage(program: &std::path::Path) {
    eprintln!(
        "usage: {} <input.jpg> <operation> [output.jpg]",
        program.display()
    );
    eprintln!("operations: 0 identity, 1 rotate90, 2 rotate180, 3 rotate270,");
    eprintln!("            4 flip-horizontal, 5 flip-vertical, 6 transpose, 7 transverse");
}
