# tiny_m87

Fine tuned VP prior diffusion model with diameter control for EHT M87 reconstruction.

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
