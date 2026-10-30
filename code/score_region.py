"""Score a region's feature grid (written by flood_api) with the trained susceptibility model,
and write the risk map next to it as a GeoTIFF for QGIS.

Usage: score_region.py <grid.npy | features.csv> <risk.csv>
Writes <risk.csv> (x, y, risk), <risk.tif> and <risk>_check.json: how far the region is from the
training data (share of cells outside the training 1-99 percentile range per feature, nearest
training AOI and its held-out AUC).
"""
import json, sys
import geopandas as gpd, joblib, numpy as np, pandas as pd, rasterio, shapely
from pathlib import Path
from rasterio.transform import from_origin

import grid_io
from paths import CACHE, OUTPUTS
from risk_model import OUT, GRID_M

RANGES = OUT / "feature_ranges.json"


def training_ranges(model_path, features):
    """1st / 99th percentile of every feature over the training samples, cached next to the model."""
    if RANGES.exists() and RANGES.stat().st_mtime >= model_path.stat().st_mtime:
        return json.loads(RANGES.read_text())
    df, _ = pd.read_pickle(CACHE / "risk_features_osm.pkl")
    r = {f: [float(df[f].quantile(0.01)), float(df[f].quantile(0.99))] for f in features}
    RANGES.write_text(json.dumps(r, indent=1))
    return r


def region_check(df, features, model_path):
    rng = training_ranges(model_path, features)
    out = np.zeros(len(df), bool)
    per = {}
    for f in features:
        lo, hi = rng[f]
        o = (df[f] < lo) | (df[f] > hi)
        per[f] = round(100 * o.mean(), 1)
        out |= o.to_numpy()
    aois = gpd.read_file(OUTPUTS / "region/study_region.gpkg", layer="aois")
    centre = shapely.Point(df.x.mean(), df.y.mean())
    d = aois.distance(centre)
    near = aois["area"].iloc[int(d.argmin())]  # column "area" = AOI code (aois.area is the geometry area)
    auc = pd.read_csv(OUT / "regions.csv").set_index("area").auc_model.get(near)
    return {"cells_outside_training_range_pct": round(100 * out.mean(), 1),
            "outside_training_range_pct_by_feature": per,
            "training_range": "1st-99th percentile of the training samples",
            "nearest_training_aoi": near, "nearest_training_aoi_km": round(float(d.min()) / 1000, 1),
            "nearest_training_aoi_held_out_auc": None if auc is None else round(float(auc), 3)}


if __name__ == "__main__":
    src, dst = Path(sys.argv[1]), Path(sys.argv[2])
    model_path = OUT / "model.joblib"
    m = joblib.load(model_path)
    df = grid_io.load(src) if src.suffix == ".npy" else pd.read_csv(src)
    df["risk"] = m["model"].predict_proba(df[m["features"]])[:, 1]
    df[["x", "y", "risk"]].to_csv(dst, index=False)
    dst.with_name(dst.stem + "_check.json").write_text(json.dumps(region_check(df, m["features"], model_path), indent=1))

    x0, y1 = df.x.min() - GRID_M / 2, df.y.max() + GRID_M / 2
    col = ((df.x - x0) // GRID_M).astype(int)
    row = ((y1 - df.y) // GRID_M).astype(int)
    arr = np.full((row.max() + 1, col.max() + 1), np.nan, dtype="float32")
    arr[row, col] = df.risk
    with rasterio.open(dst.with_suffix(".tif"), "w", driver="GTiff", height=arr.shape[0], width=arr.shape[1], count=1,
                       dtype="float32", crs="EPSG:3035", transform=from_origin(x0, y1, GRID_M, GRID_M), nodata=np.nan,
                       compress="deflate") as out:
        out.write(arr, 1)
