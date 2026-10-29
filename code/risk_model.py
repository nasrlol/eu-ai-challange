"""Flood susceptibility model: how likely each place near the July 2021 floods was to flood,
from terrain and hydrology alone.

Training data: random points inside each CEMS area of interest, labelled flooded / not flooded
by the observed extent. Validation is leave-one-area-out, so every score and risk map for an
area comes from a model that never saw that area.
"""
import argparse, re, urllib.request
import joblib, numpy as np, pandas as pd, geopandas as gpd, rasterio, shapely
from pathlib import Path
from pyproj import Transformer
from rasterio.merge import merge
from rasterio.transform import from_origin
from rasterio.windows import Window, from_bounds
from scipy.ndimage import minimum_filter
from scipy.spatial import cKDTree
from sklearn.ensemble import HistGradientBoostingClassifier
from sklearn.inspection import permutation_importance
from sklearn.metrics import roc_auc_score

from main import DATA
from paths import CACHE, OUTPUTS

DEM_DIR = DATA / "dem"
DEM_URL = "https://copernicus-dem-30m.s3.amazonaws.com/{0}/{0}.tif"
EFAS = {f"rp{rp}": DATA / f"cems-efas/Europe_RP{rp}_filled_depth.tif" for rp in (10, 100, 500)}
FEATURES = ["elev", "slope", "relief", "hand", "dist_river", "rp10", "rp100", "rp500"]
# candidate extras (evaluate.py tests them; the API contract in notes/08 is FEATURES only)
EXTRA = ["relief_300", "relief_3k", "hand_major", "dist_major"]
OUT = OUTPUTS / "risk"
TO_LONLAT = Transformer.from_crs(3035, 4326, always_xy=True)
GRID_M = 100  # risk map resolution
RIVERS = "osm"  # river source for hand / dist_river: "osm" (same as flood_api, works anywhere) or "cems" (AOIs only)


def dem_tiles(w, s, e, n):
    """Copernicus DEM 30 m tiles covering the bounds, downloaded once into working_data/dem."""
    paths = []
    for lat in range(int(np.floor(s)), int(np.floor(n)) + 1):
        for lon in range(int(np.floor(w)), int(np.floor(e)) + 1):
            name = f"Copernicus_DSM_COG_10_N{lat:02d}_00_E{lon:03d}_00_DEM"
            path = DEM_DIR / f"{name}.tif"
            if not path.exists():
                print(f"  downloading {name}", flush=True)
                DEM_DIR.mkdir(exist_ok=True)
                urllib.request.urlretrieve(DEM_URL.format(name), path.with_suffix(".part"))
                path.with_suffix(".part").rename(path)
            paths.append(path)
    return paths


def sample_in(geom, n, rng, exclude=None):
    """n uniform random points inside geom (EPSG:3035), part by part so tiny polygons still get hits."""
    parts = [p for p in getattr(geom, "geoms", [geom]) if p.area > 0]
    counts = rng.multinomial(n, np.array([p.area for p in parts]) / sum(p.area for p in parts))
    out = []
    for part, k in zip(parts, counts):
        shapely.prepare(part)
        got = np.empty((0, 2))
        x0, y0, x1, y1 = part.bounds
        for _ in range(50):
            if len(got) >= k:
                break
            xy = rng.uniform((x0, y0), (x1, y1), (max(4 * k, 64), 2))
            ok = shapely.contains_xy(part, xy[:, 0], xy[:, 1])
            if exclude is not None:
                ok &= ~shapely.contains_xy(exclude, xy[:, 0], xy[:, 1])
            got = np.vstack([got, xy[ok]])
        out.append(got[:k])
    return np.vstack(out)


class Area:
    """A region with every raster needed to compute features anywhere inside it: either a CEMS
    area of interest (its observed flood gives the labels) or any polygon (EPSG:3035) to score."""

    def __init__(self, folder=None, rivers=RIVERS, region=None, name="region"):
        def read(kind):
            path = next(iter(sorted(folder.glob(f"*_{kind}_*.shp"))), None)
            return gpd.read_file(path).to_crs(3035) if path else gpd.GeoDataFrame(geometry=[], crs=3035)

        if folder is not None:
            self.name = "_".join(re.match(r"(EMSR\d+)_(AOI\d+)_(DEL|GRA)", folder.name).groups())
            self.aoi = read("areaOfInterestA").union_all()
            self.flood = read("observedEventA").union_all()
        else:
            self.name, self.aoi, self.flood = name, region, shapely.Polygon()
        if rivers == "cems":
            water, lines = read("hydrographyA"), read("hydrographyL").geometry
            major = lines
        else:
            import osm_data
            water, ww = osm_data.layer("water", self.aoi), osm_data.layer("waterways", self.aoi)
            lines, major = ww.geometry, ww[ww.waterway == "river"].geometry
        self.water = water.union_all() if len(water) else None
        for g in (self.aoi, self.flood, self.water):
            if g is not None:
                shapely.prepare(g)

        # Terrain: DEM mosaic over the AOI, plus slope and height above the lowest point within ~1 km
        w, s, e, n = gpd.GeoSeries([self.aoi], crs=3035).to_crs(4326).total_bounds + [-0.02, -0.02, 0.02, 0.02]
        srcs = [rasterio.open(p) for p in dem_tiles(w, s, e, n)]
        dem, self.tf = merge(srcs, bounds=(w, s, e, n), res=(1 / 3600, 1 / 3600), nodata=np.nan)
        for src in srcs:
            src.close()
        self.dem = dem[0].astype("float32")
        dy = 111_320 / 3600
        gy, gx = np.gradient(self.dem, dy, dy * np.cos(np.radians((s + n) / 2)))
        self.slope = np.degrees(np.arctan(np.hypot(gx, gy))).astype("float32")
        self.relief = self.dem - minimum_filter(self.dem, size=33)
        self.relief_300 = self.dem - minimum_filter(self.dem, size=11)
        self.relief_3k = self.dem - minimum_filter(self.dem, size=99)

        # Rivers: river lines + water body outlines, densified to 50 m; river surface = local DEM minimum
        rivers = pd.concat([lines, water.boundary]).segmentize(50)
        xy = np.unique(np.round(shapely.get_coordinates(rivers.values) / 25) * 25, axis=0)
        if len(xy) == 0:
            raise ValueError(f"{self.name}: no rivers found")
        self.river_tree = cKDTree(xy)
        dem_min5 = minimum_filter(self.dem, size=5)
        self.river_z = self.at(dem_min5, self.tf, *TO_LONLAT.transform(xy[:, 0], xy[:, 1]))
        # Major rivers only (OSM waterway=river + open water), for the candidate EXTRA features
        mxy = np.unique(np.round(shapely.get_coordinates(pd.concat([major, water.boundary]).segmentize(50).values) / 25) * 25, axis=0)
        if len(mxy) == 0:
            mxy = xy
        self.major_tree = cKDTree(mxy)
        self.major_z = self.at(dem_min5, self.tf, *TO_LONLAT.transform(mxy[:, 0], mxy[:, 1]))

        # Modelled hazard: EFAS return-period depth maps
        self.efas = {}
        for k, path in EFAS.items():
            with rasterio.open(path) as src:
                win = from_bounds(w, s, e, n, src.transform)
                # whole pixels, so the data read and its transform line up (a fractional window shifted EFAS by up to 1 px)
                win = Window(int(np.floor(win.col_off)), int(np.floor(win.row_off)), int(np.ceil(win.width)) + 1, int(np.ceil(win.height)) + 1)
                arr = src.read(1, window=win).astype("float32")
                self.efas[k] = (np.where(arr > 0, arr, 0), src.window_transform(win))

    def depth(self, rp, xy, dem_check=True):
        """EFAS depth (m) at points. With dem_check, the 90 m EFAS value is checked against the 30 m DEM:
        water surface = lowest ground within ~75 m + EFAS depth, ground at the point = lowest DEM
        within ~45 m (drops roofs and trees); a point above the water surface is dry."""
        lon, lat = TO_LONLAT.transform(xy[:, 0], xy[:, 1])
        arr, tf = self.efas[rp]
        d = self.at(arr, tf, lon, lat)
        if not dem_check:
            return d
        if not hasattr(self, "_base"):
            self._base, self._ground = minimum_filter(self.dem, size=5), minimum_filter(self.dem, size=3)
        above = self.at(self._ground, self.tf, lon, lat) - self.at(self._base, self.tf, lon, lat)
        return np.where(d > 0, np.maximum(d - above, 0), 0).astype("float32")

    @staticmethod
    def at(arr, tf, lon, lat):
        col, row = ~tf * (np.asarray(lon), np.asarray(lat))
        row = np.clip(np.floor(row).astype(int), 0, arr.shape[0] - 1)
        col = np.clip(np.floor(col).astype(int), 0, arr.shape[1] - 1)
        return arr[row, col]

    def features(self, xy):
        lon, lat = TO_LONLAT.transform(xy[:, 0], xy[:, 1])
        elev = self.at(self.dem, self.tf, lon, lat)
        dist, i = self.river_tree.query(xy)
        mdist, j = self.major_tree.query(xy)
        f = dict(elev=elev, slope=self.at(self.slope, self.tf, lon, lat), relief=self.at(self.relief, self.tf, lon, lat),
                 hand=elev - self.river_z[i], dist_river=dist,
                 relief_300=self.at(self.relief_300, self.tf, lon, lat), relief_3k=self.at(self.relief_3k, self.tf, lon, lat),
                 hand_major=elev - self.major_z[j], dist_major=mdist)
        f.update({k: self.at(arr, tf, lon, lat) for k, (arr, tf) in self.efas.items()})
        return pd.DataFrame(f).astype("float32")

    def samples(self, n, rng):
        """n flooded + n dry points; permanent water is left out of both."""
        wet = sample_in(self.flood, n, rng, exclude=self.water)
        dry_zone = self.flood if self.water is None else shapely.union(self.flood, self.water)
        dry = sample_in(self.aoi, n, rng, exclude=dry_zone)
        df = self.features(np.vstack([wet, dry]))
        df["flooded"] = np.r_[np.ones(len(wet)), np.zeros(len(dry))].astype(int)
        df["area"] = self.name
        return df

    def grid(self):
        """Every GRID_M cell centre inside the AOI, for the risk map."""
        x0, y0, x1, y1 = self.aoi.bounds
        xs, ys = np.arange(x0 + GRID_M / 2, x1, GRID_M), np.arange(y1 - GRID_M / 2, y0, -GRID_M)
        gx, gy = np.meshgrid(xs, ys)
        inside = shapely.contains_xy(self.aoi, gx, gy)
        if self.water is not None:
            inside &= ~shapely.contains_xy(self.water, gx, gy)
        xy = np.c_[gx[inside], gy[inside]]
        return dict(feats=self.features(xy), flooded=shapely.contains_xy(self.flood, xy[:, 0], xy[:, 1]),
                    mask=inside, transform=from_origin(x0, y1, GRID_M, GRID_M))


def build(folders, n, seed, rivers=RIVERS):
    rng = np.random.default_rng(seed)
    samples, grids = [], {}
    for folder in folders:
        area = Area(folder, rivers)
        print(f"{area.name}: features", flush=True)
        samples.append(area.samples(n, rng))
        grids[area.name] = area.grid()
        del area
    return pd.concat(samples, ignore_index=True), grids


def new_model():
    return HistGradientBoostingClassifier(max_iter=300, learning_rate=0.05, max_leaf_nodes=31,
                                          l2_regularization=1.0, random_state=0)


def write_map(g, risk, path):
    arr = np.full(g["mask"].shape, np.nan, dtype="float32")
    arr[g["mask"]] = risk
    with rasterio.open(path, "w", driver="GTiff", height=arr.shape[0], width=arr.shape[1], count=1, dtype="float32",
                       crs="EPSG:3035", transform=g["transform"], nodata=np.nan, compress="deflate") as dst:
        dst.write(arr, 1)


if __name__ == "__main__":
    p = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    p.add_argument("-n", type=int, default=3000, help="flooded and dry sample points per area")
    p.add_argument("--rebuild", action="store_true", help="recompute features instead of using the cache")
    p.add_argument("--rivers", choices=["osm", "cems"], default=RIVERS,
                   help=f"river source; only the default ({RIVERS}) writes maps and the model, others just report scores")
    args = p.parse_args()
    write = args.rivers == RIVERS
    OUT.mkdir(parents=True, exist_ok=True); CACHE.mkdir(exist_ok=True)

    cache = CACHE / f"risk_features_{args.rivers}.pkl"
    if cache.exists() and not args.rebuild:
        df, grids = pd.read_pickle(cache)
    else:
        folders = sorted(f for d in sorted(DATA.glob("ems_*")) for f in d.glob("*_PRODUCT_*_vector"))
        df, grids = build(folders, args.n, seed=0, rivers=args.rivers)
        pd.to_pickle((df, grids), cache)

    # Leave-one-area-out: train on all other areas, score and map the held-out one
    rows = []
    for name, g in grids.items():
        train, test = df[df.area != name], df[df.area == name]
        model = new_model().fit(train[FEATURES], train.flooded)
        rows.append(dict(
            area=name,
            auc_model=roc_auc_score(test.flooded, model.predict_proba(test[FEATURES])[:, 1]),
            auc_rp100=roc_auc_score(test.flooded, test.rp100),
            auc_hand=roc_auc_score(test.flooded, -test.hand),
        ))
        risk = model.predict_proba(g["feats"][FEATURES])[:, 1]
        if write:
            write_map(g, risk, OUT / f"risk_{name}.tif")
        rows[-1].update(
            area_km2=g["mask"].sum() * GRID_M ** 2 / 1e6,
            flooded_pct=100 * g["flooded"].mean(),
            mean_risk=risk.mean(),
            high_risk_pct=100 * (risk >= 0.5).mean(),
            high_risk_hit_pct=100 * (risk[g["flooded"]] >= 0.5).mean() if g["flooded"].any() else np.nan,
        )
    res = pd.DataFrame(rows).sort_values("mean_risk", ascending=False)
    final = new_model().fit(df[FEATURES], df.flooded)
    if write:
        res.to_csv(OUT / "regions.csv", index=False)
        joblib.dump(dict(model=final, features=FEATURES, rivers=args.rivers), OUT / "model.joblib")
    sub = df.sample(min(len(df), 20_000), random_state=0)
    imp = permutation_importance(final, sub[FEATURES], sub.flooded, scoring="roc_auc", n_repeats=5, random_state=0)

    pd.set_option("display.width", 160)
    print("\nHeld-out performance per area (ROC AUC: 0.5 = random, 1.0 = perfect)")
    print(res.round(3).to_string(index=False))
    print(f"\nMedian AUC  model {res.auc_model.median():.3f} | RP100 depth alone {res.auc_rp100.median():.3f} "
          f"| height above river alone {res.auc_hand.median():.3f}")
    print("\nFeature importance (drop in AUC when shuffled)")
    print(pd.Series(imp.importances_mean, FEATURES).sort_values(ascending=False).round(3).to_string())
    print(f"\nWrote {OUT}/regions.csv, model.joblib and risk_*.tif maps" if write else
          f"\nRivers = {args.rivers}: scores only, nothing written")
