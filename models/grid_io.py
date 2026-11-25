"""Binary format for the Rust -> Python feature handoff (replaces JSON).

One row per 100 m grid cell, stored as a NumPy structured array in a .npy file:
x, y (EPSG:3035 cell centre, float64) followed by risk_model.FEATURES (float32), little-endian.
No parsing: Python memory-maps the file, Rust writes a fixed header plus raw rows.

The header descr Rust must write (keep in sync with FEATURES):
    [('x', '<f8'), ('y', '<f8'), ('elev', '<f4'), ('slope', '<f4'), ('relief', '<f4'), ('hand', '<f4'),
     ('dist_river', '<f4'), ('rp10', '<f4'), ('rp100', '<f4'), ('rp500', '<f4')]
"""
import io
import numpy as np, pandas as pd

from risk_model import FEATURES

GRID_DTYPE = np.dtype([("x", "<f8"), ("y", "<f8")] + [(f, "<f4") for f in FEATURES])


def to_frame(arr):
    """Structured array -> DataFrame with the columns the model expects."""
    if arr.dtype != GRID_DTYPE:
        raise ValueError(f"unexpected layout {arr.dtype}, expected {GRID_DTYPE}")
    return pd.DataFrame({name: arr[name] for name in GRID_DTYPE.names})


def from_frame(df):
    arr = np.empty(len(df), GRID_DTYPE)
    for name in GRID_DTYPE.names:
        arr[name] = df[name].to_numpy()
    return arr


def load(path):
    return to_frame(np.load(path, mmap_mode="r"))


def save(df, path):
    np.save(path, from_frame(df))


def from_bytes(buf):
    """For an HTTP body (Content-Type: application/octet-stream) instead of a file."""
    return to_frame(np.load(io.BytesIO(buf)))


def to_bytes(df):
    out = io.BytesIO()
    np.save(out, from_frame(df))
    return out.getvalue()
