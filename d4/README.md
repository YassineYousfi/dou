# Rotate and Flip

You're on a desert island with only 2.50 MiB of RAM, you need to rotate an flip a 3024x4032 JPEG image.
Your life depends on it. This program will do it for you. Phew.

```text
cargo run --release -- IMG_9885.jpg 1
```

| Operation | Result |
| ---: | --- |
| 0 | identity |
| 1 | 90 degrees clockwise |
| 2 | 180 degrees |
| 3 | 270 degrees clockwise |
| 4 | horizontal flip |
| 5 | vertical flip |
| 6 | transpose |
| 7 | transverse |

## How?

Use the symmetries of DCT bases to perform the operations directly in the DCT domain, without ever decompressing the image.
MCU by MCU, so you don't even need to load the whole image into memory.

## Benchmark

| | This | ImageMagick `convert -rotate 90` |
| --- | ---: | ---: |
| Wall time | 0.31 s | 0.24 s |
| Peak RSS | 2.50 MiB | 197.02 MiB |
