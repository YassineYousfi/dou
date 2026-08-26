from __future__ import annotations

import math

import torch

from models.precond import VPPrecond


class FixedDiameterVP(VPPrecond):

    def __init__(
        self,
        diameter_px: float,
        diameter_mean: float = 17.30470085144043,
        diameter_scale: float = 3.123326301574707,
        cfg_scale: float = 1.0,
        label_dim: int = 2,
        **model_kwargs,
    ) -> None:
        if label_dim != 2:
            raise ValueError("the diameter-conditioned checkpoint requires label_dim=2")
        if not math.isfinite(float(diameter_px)):
            raise ValueError("diameter_px must be finite")
        if not math.isfinite(float(diameter_mean)):
            raise ValueError("diameter_mean must be finite")
        if not math.isfinite(float(diameter_scale)) or diameter_scale <= 0:
            raise ValueError("diameter_scale must be finite and positive")
        if not math.isfinite(float(cfg_scale)):
            raise ValueError("cfg_scale must be finite")

        super().__init__(label_dim=label_dim, **model_kwargs)
        self.diameter_px = float(diameter_px)
        self.diameter_mean = float(diameter_mean)
        self.diameter_scale = float(diameter_scale)
        self.cfg_scale = float(cfg_scale)

    def forward(
        self,
        x: torch.Tensor,
        sigma: torch.Tensor,
        class_labels: torch.Tensor | None = None,
        force_fp32: bool = False,
        **model_kwargs,
    ) -> torch.Tensor:
        del class_labels
        normalized = (self.diameter_px - self.diameter_mean) / self.diameter_scale
        condition = x.new_tensor((normalized, 1.0)).expand(x.shape[0], -1)
        conditional = super().forward(
            x,
            sigma,
            condition,
            force_fp32=force_fp32,
            **model_kwargs,
        )
        if self.cfg_scale == 1.0:
            return conditional

        null_condition = x.new_zeros((x.shape[0], 2))
        unconditional = super().forward(
            x,
            sigma,
            null_condition,
            force_fp32=force_fp32,
            **model_kwargs,
        )
        return unconditional + self.cfg_scale * (conditional - unconditional)
