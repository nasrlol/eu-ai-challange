"""Score a region's feature table (written by flood_api) with the trained susceptibility model,
and write the risk map next to it as a GeoTIFF for QGIS."""
import sys
import joblib, numpy as np, pandas as pd, rasterio
from pathlib import Path
from rasterio.transform import from_origin

from risk_model import OUT, GRID_M

src, dst = Path(sys.argv[1]), Path(sys.argv[2])
m = joblib.load(OUT / "model.joblib")
df = pd.read_csv(src)
df["risk"] = m["model"].predict_proba(df[m["features"]])[:, 1]
df.to_csv(dst, index=False)

x0, y1 = df.x.min() - GRID_M / 2, df.y.max() + GRID_M / 2
col = ((df.x - x0) // GRID_M).astype(int)
row = ((y1 - df.y) // GRID_M).astype(int)
arr = np.full((row.max() + 1, col.max() + 1), np.nan, dtype="float32")
arr[row, col] = df.risk
with rasterio.open(dst.with_suffix(".tif"), "w", driver="GTiff", height=arr.shape[0], width=arr.shape[1], count=1,
                   dtype="float32", crs="EPSG:3035", transform=from_origin(x0, y1, GRID_M, GRID_M), nodata=np.nan,
                   compress="deflate") as out:
    out.write(arr, 1)
