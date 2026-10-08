# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors
"""assets/icon-browser.png from assets/icon.png: the same F, white on an
orange tile, for the browser a profile runs in.

The application and a profile's browser had the same icon, so a Dock with one
of each showed two identical tiles and nobody could tell which was which
(tester, 08.10.2026). The flame is told from the dark tile by its colour
saturation, which also carries the soft edges across.
"""
import pathlib
import numpy as np
from PIL import Image

root = pathlib.Path(__file__).resolve().parent
src = np.asarray(Image.open(root / "icon.png").convert("RGBA")).astype(np.float32) / 255.0
rgb, alpha = src[..., :3], src[..., 3:]
mx, mn = rgb.max(axis=2), rgb.min(axis=2)
sat = np.where(mx > 0, (mx - mn) / np.maximum(mx, 1e-6), 0.0)
flame = np.clip((sat - 0.35) / 0.35, 0, 1) * np.clip((mx - 0.25) / 0.35, 0, 1)
flame = flame[..., None]

h = rgb.shape[0]
t = np.linspace(0, 1, h)[:, None, None]
top = np.array([1.00, 0.70, 0.00])   # #FFB300
bottom = np.array([0.93, 0.26, 0.08])  # #ED4214
tile = top * (1 - t) + bottom * t
tile = np.broadcast_to(tile, rgb.shape)

out_rgb = tile * (1 - flame) + np.ones_like(rgb) * flame
out = np.concatenate([out_rgb, alpha], axis=2)
Image.fromarray((out * 255).round().astype(np.uint8)).save(root / "icon-browser.png")
print("wrote", root / "icon-browser.png")
