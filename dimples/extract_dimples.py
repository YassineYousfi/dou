#!/usr/bin/env python3
from argparse import ArgumentParser
from pathlib import Path

import numpy as np
from PIL import Image
from scipy.signal import wiener


JPEG_PERIOD = 8
AVERAGE_BLOCK_SIZE = 32


def extract_fingerprint(image: np.ndarray) -> tuple[np.ndarray, int]:
    """Return the paper's averaged 32x32 RGB noise-residual block."""
    residual = np.zeros(image.shape[:2], dtype=np.float64)
    for channel in range(3):
        values = image[:, :, channel].astype(np.float64)
        with np.errstate(divide="ignore", invalid="ignore"):
            denoised = wiener(values, (3, 3))
        residual += values - denoised
    residual /= 3

    height = residual.shape[0] // AVERAGE_BLOCK_SIZE * AVERAGE_BLOCK_SIZE
    width = residual.shape[1] // AVERAGE_BLOCK_SIZE * AVERAGE_BLOCK_SIZE
    if height == 0 or width == 0:
        raise ValueError("the image must be at least 32x32 pixels")

    blocks = residual[:height, :width].reshape(
        height // AVERAGE_BLOCK_SIZE,
        AVERAGE_BLOCK_SIZE,
        width // AVERAGE_BLOCK_SIZE,
        AVERAGE_BLOCK_SIZE,
    )
    fingerprint = blocks.mean(axis=(0, 2))
    block_count = (height // AVERAGE_BLOCK_SIZE) * (width // AVERAGE_BLOCK_SIZE)
    return fingerprint, block_count


def dimple_pce(fingerprint: np.ndarray) -> tuple[float, tuple[int, int], float]:
    template = np.zeros_like(fingerprint)
    template[::JPEG_PERIOD, ::JPEG_PERIOD] = 1.0

    fingerprint = fingerprint - fingerprint.mean()
    template = template - template.mean()
    fingerprint /= np.linalg.norm(fingerprint)
    template /= np.linalg.norm(template)

    correlation = np.empty((JPEG_PERIOD, JPEG_PERIOD))
    for row in range(JPEG_PERIOD):
        for column in range(JPEG_PERIOD):
            shifted_template = np.roll(template, (row, column), axis=(0, 1))
            correlation[row, column] = np.sum(fingerprint * shifted_template)

    energy = correlation**2
    peak = np.unravel_index(np.argmax(energy), energy.shape)
    background_energy = (energy.sum() - energy[peak]) / (energy.size - 1)
    pce = float(energy[peak] / background_energy)
    return pce, peak, float(correlation[peak])


def main() -> None:
    parser = ArgumentParser(description=__doc__)
    parser.add_argument("image", type=Path, nargs="?", default=Path("IMG_9885.jpg"))
    parser.add_argument(
        "--output-prefix",
        type=Path,
        default=Path("dimples"),
        help="output path without an extension (default: dimples)",
    )
    args = parser.parse_args()

    with Image.open(args.image) as source:
        image = np.asarray(source.convert("RGB"))

    fingerprint, block_count = extract_fingerprint(image)
    pce, (row, column), correlation = dimple_pce(fingerprint)

    pixels = 255 * (fingerprint - fingerprint.min()) / np.ptp(fingerprint)
    preview = Image.fromarray(pixels.astype(np.uint8))
    preview.resize((512, 512), Image.Resampling.NEAREST).save(
        args.output_prefix.with_suffix(".png")
    )

    polarity = "bright" if correlation > 0 else "dark"
    print(f"averaged {block_count} non-overlapping 32x32 blocks")
    print(f"dimple phase: row {row}, column {column} (zero-indexed)")
    print(f"peak residual polarity: {polarity}")
    print(f"PCE: {pce:.2f}")


if __name__ == "__main__":
    main()
