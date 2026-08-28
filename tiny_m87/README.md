# tiny_m87*

Fine tuned VP prior diffusion model with diameter control for EHT M87 reconstruction.

https://github.com/user-attachments/assets/125cf835-15c2-42ab-bd49-c25db772b322

## Set up

```bash
cd tiny_m87
git lfs install
git submodule update --init --recursive
git lfs pull
uv sync
```

## Reconstruct at a fixed diameter

```bash
cd InverseBench
INVERSEBENCH_BLACKHOLE_DATA=/path/to/blackhole \
PYTHONPATH=.. \
uv run --project .. python main.py \
  --config-dir ../configs \
  --config-name tiny_m87 \
  diameter_px=18.0
```

<img width="768" height="4096" alt="paired-with-original" src="https://github.com/user-attachments/assets/dd6fcdf1-9ef9-4f18-8cda-35af3c973e6e" />
