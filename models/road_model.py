"""Roads under future floods.

Roads (CEMS transportationL) are cut into ~50 m pieces and checked against:
  - EFAS flood scenarios RP10 / RP100 / RP500 (impassable where depth >= 0.3 m)
  - the ML susceptibility maps from risk_model.py (high risk where score >= 0.5)
  - the observed 2021 extent (reference)
  - the in-between extents from interp_model.py (closure timeline)
A road-damage model is trained on the damage-grading maps (damage_gra) using only inputs that
exist before a flood, validated leave-one-area-out, then applied to every road.
"""
import argparse
import numpy as np, pandas as pd, geopandas as gpd, rasterio, shapely
from pathlib import Path
from shapely.ops import substring
from sklearn.ensemble import HistGradientBoostingClassifier
from sklearn.metrics import roc_auc_score

from main import DATA
from paths import CACHE, OUTPUTS
from risk_model import Area, FEATURES as TERRAIN
import risk_model, interp_model

OUT = OUTPUTS / "roads"
PIECE_M = 50
IMPASSABLE_M = 0.3   # water depth at which a road is treated as closed to cars
HIGH_RISK = 0.5
CLASSES = {"2111": "Highway", "21120": "Primary road", "21121": "Secondary road", "21122": "Local road",
           "21124": "Cart track", "2121": "Railway", "2141": "Bridge", "2130": "Airfield"}
DAMAGE = {"No visible damage": 0, "Possibly damaged": 1, "Damaged": 1, "Destroyed": 1}
MODEL_FEATURES = TERRAIN + ["risk", "road_code"]


def pieces(roads):
    """Cut every line into ~PIECE_M pieces; returns one row per piece with its midpoint and length."""
    rows = []
    for i, geom in enumerate(roads.geometry):
        for line in getattr(geom, "geoms", [geom]):
            n = max(1, int(np.ceil(line.length / PIECE_M)))
            step = line.length / n
            for k in range(n):
                rows.append((i, substring(line, k * step, (k + 1) * step), step))
    idx, geoms, lengths = zip(*rows)
    out = roads.iloc[list(idx)].drop(columns="geometry").reset_index(drop=True)
    out["length_m"] = lengths
    return gpd.GeoDataFrame(out, geometry=list(geoms), crs=3035)


def sample_raster(path, xy, band=1):
    with rasterio.open(path) as src:
        arr = src.read(band)
        col, row = ~src.transform * (xy[:, 0], xy[:, 1])
        row, col = np.floor(row).astype(int), np.floor(col).astype(int)
        ok = (row >= 0) & (row < arr.shape[0]) & (col >= 0) & (col < arr.shape[1])
        v = np.full(len(xy), np.nan, "float32")
        v[ok] = arr[row[ok], col[ok]]
        return np.where(v == src.nodata, np.nan, v) if src.nodata is not None and not np.isnan(src.nodata) else v


def build(folders):
    out = []
    for folder in folders:
        path = next(iter(folder.glob("*_transportationL_*.shp")), None)
        if path is None:
            continue
        area = Area(folder)
        print(f"{area.name}: roads", flush=True)
        roads = gpd.read_file(path).to_crs(3035)
        roads = roads[roads.geometry.notna() & ~roads.geometry.is_empty]
        p = pieces(roads)
        mid = shapely.get_coordinates(p.geometry.interpolate(0.5, normalized=True))
        p = pd.concat([p, area.features(mid)], axis=1)
        p["area"] = area.name
        p["road_class"] = p["info"].str.split("-").str[0].map(CLASSES).fillna("Other")
        p["observed"] = shapely.contains_xy(area.flood, mid[:, 0], mid[:, 1])
        risk_tif = risk_model.OUT / f"risk_{area.name}.tif"
        p["risk"] = sample_raster(risk_tif, mid) if risk_tif.exists() else np.nan
        p["damage"] = p["damage_gra"].map(DAMAGE) if "damage_gra" in p else np.nan
        # Closure timeline: state / flip-hour rasters from interp_model.py
        for tif in sorted(interp_model.OUT.glob(f"{area.name}_*.tif")):
            key = tif.stem[len(area.name) + 1:]
            p[f"state_{key}"] = sample_raster(tif, mid, 1)
            p[f"flip_{key}"] = sample_raster(tif, mid, 2)
        out.append(p)
        del area
    return pd.concat(out, ignore_index=True)


def km(mask, df):
    return df.loc[mask, "length_m"].sum() / 1000


def new_model():
    return HistGradientBoostingClassifier(max_iter=200, learning_rate=0.05, l2_regularization=1.0,
                                          categorical_features=[MODEL_FEATURES.index("road_code")], random_state=0)


if __name__ == "__main__":
    p = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    p.add_argument("--rebuild", action="store_true", help="recompute road pieces instead of using the cache")
    p.add_argument("--step", type=float, default=6, help="hours between closure timeline steps")
    args = p.parse_args()
    OUT.mkdir(parents=True, exist_ok=True); CACHE.mkdir(exist_ok=True)

    cache = CACHE / "road_pieces.pkl"
    if cache.exists() and not args.rebuild:
        df = pd.read_pickle(cache)
    else:
        df = build(sorted(f for d in sorted(DATA.glob("ems_*")) for f in d.glob("*_PRODUCT_*_vector")))
        df.to_pickle(cache)
    df = pd.DataFrame(df)  # plain frame: on a GeoDataFrame `.area` is geometric area, not our column
    df["road_class"] = df["road_class"].astype("category")
    df["road_code"] = df["road_class"].cat.codes

    # 1. Exposure under each scenario, per area and road class
    scen = {"Observed 2021": df.observed, "RP10": df.rp10 >= IMPASSABLE_M, "RP100": df.rp100 >= IMPASSABLE_M,
            "RP500": df.rp500 >= IMPASSABLE_M, "ML high risk": df.risk >= HIGH_RISK}
    by_area = pd.DataFrame({k: df[m].groupby("area").length_m.sum() / 1000 for k, m in scen.items()}).fillna(0)
    by_area.insert(0, "Road km", df.groupby("area").length_m.sum() / 1000)
    by_class = pd.DataFrame({k: df[m].groupby("road_class", observed=True).length_m.sum() / 1000 for k, m in scen.items()}).fillna(0)
    by_class.insert(0, "Road km", df.groupby("road_class", observed=True).length_m.sum() / 1000)
    by_area.round(2).to_csv(OUT / "exposure_by_area.csv"); by_class.round(2).to_csv(OUT / "exposure_by_class.csv")

    # How well each scenario covers the roads that actually flooded in 2021
    hit = pd.DataFrame({k: dict(flagged_km=km(m, df), share_of_2021_flooded_km=km(m & df.observed, df) / km(df.observed, df))
                        for k, m in scen.items() if k != "Observed 2021"}).T

    # 2. Road damage model: grading-map labels, pre-flood inputs only, leave-one-area-out
    lab = df[df.damage.notna()]
    lab = lab[lab.groupby("area").damage.transform("nunique") == 2]  # areas with both damaged and undamaged roads
    cv = []
    for name in lab.area.unique():
        tr, te = lab[lab.area != name], lab[lab.area == name]
        model = new_model().fit(tr[MODEL_FEATURES], tr.damage)
        cv.append(dict(area=name, pieces=len(te), damaged_pct=100 * te.damage.mean(),
                       auc_model=roc_auc_score(te.damage, model.predict_proba(te[MODEL_FEATURES])[:, 1]),
                       auc_rp100=roc_auc_score(te.damage, te.rp100), auc_risk=roc_auc_score(te.damage, te.risk.fillna(0)),
                       auc_observed=roc_auc_score(te.damage, te.observed)))
    cv = pd.DataFrame(cv)
    cv.round(3).to_csv(OUT / "damage_validation.csv", index=False)
    final = new_model().fit(lab[MODEL_FEATURES], lab.damage)
    df["damage_prob"] = final.predict_proba(df[MODEL_FEATURES])[:, 1]

    # 3. Closure timeline: road km under water at each step between PRODUCT and MONIT01
    timeline, ts = [], pd.read_csv(interp_model.OUT / "timeseries.csv")
    for col in [c for c in df.columns if c.startswith("state_")]:
        key = col[6:]
        # Several areas can share the same date pair, so split by area
        for name, d in df[df[col].notna() & (df[col] >= 0)].groupby("area"):
            pair = f"{name}_{key}"
            state, flip, end = d[col], d[f"flip_{key}"], ts.loc[ts.pair == pair, "hours"].max()
            for h in sorted(set(np.r_[np.arange(0, end, args.step), end])):
                wet = (state == 1) | ((state == 2) & (flip > h)) | ((state == 3) & (flip <= h))
                timeline.append(dict(pair=pair, hours=h, road_km_flooded=km(wet, d),
                                     main_road_km_flooded=km(wet & d.road_class.isin(["Highway", "Primary road", "Secondary road", "Bridge"]), d)))
    pd.DataFrame(timeline).round(3).to_csv(OUT / "closure_timeline.csv", index=False)

    keep = ["area", "road_class", "name", "length_m", "observed", "rp10", "rp100", "rp500", "risk", "damage_gra", "damage_prob", "geometry"]
    gdf = gpd.GeoDataFrame(df[[c for c in keep if c in df]], geometry="geometry", crs=3035)
    gdf[(gdf.rp500 > 0) | (gdf.risk >= HIGH_RISK) | gdf.observed | (gdf.damage_prob >= 0.5)].to_file(OUT / "roads_at_risk.gpkg", driver="GPKG")

    pd.set_option("display.width", 160)
    print(f"\nRoad km impassable (depth ≥ {IMPASSABLE_M} m) or high risk, by class")
    print(by_class.round(1).to_string())
    print("\nScenario vs 2021: km flagged, and share of the roads that flooded in 2021 it catches")
    print(hit.round(2).to_string())
    print("\nRoad damage model, leave-one-area-out ROC AUC (pre-flood inputs only)")
    print(cv.round(3).to_string(index=False))
    print(f"Median AUC: model {cv.auc_model.median():.3f} | RP100 {cv.auc_rp100.median():.3f} | "
          f"ML risk {cv.auc_risk.median():.3f} | observed 2021 extent {cv.auc_observed.median():.3f}")
    print(f"\nWrote {OUT}/: exposure_by_area.csv, exposure_by_class.csv, damage_validation.csv, closure_timeline.csv, roads_at_risk.gpkg")
