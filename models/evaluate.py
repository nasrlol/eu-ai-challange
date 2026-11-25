"""How good are the models? Everything is scored on data the model did not see.

1. Risk model on the full grid of each held-out AOI (real flood share, not the balanced training
   sample): ROC AUC, average precision, and precision / recall when flagging the top 5 % / 10 % of
   the area. Compared with the EFAS RP100 map and with the CEMS-river variant.
2. Building damage: do depth (EFAS + DEM check) and terrain risk separate damaged from undamaged
   buildings in the CEMS grading maps (builtUpP damage_gra)?
3. Collects the earlier checks (roads, DEM depth check, in-between model) into one table.
Writes outputs/eval/summary.csv and per-part CSVs.
"""
import argparse
import geopandas as gpd, numpy as np, pandas as pd, shapely
from sklearn.metrics import average_precision_score, roc_auc_score

import risk_model
from main import DATA
from paths import CACHE, OUTPUTS
from risk_model import FEATURES, Area, new_model
from road_model import sample_raster

OUT = OUTPUTS / "eval"
DAMAGED = {"Damaged", "Destroyed", "Possibly damaged"}


def top_share(score, truth, share):
    """Precision and recall when the highest-scoring `share` of cells is flagged."""
    k = max(1, int(len(score) * share))
    flag = np.zeros(len(score), bool)
    flag[np.argsort(-score)[:k]] = True
    return (truth & flag).sum() / k, (truth & flag).sum() / max(truth.sum(), 1)


def grid_scores(name, score, truth):
    row = dict(area=name, flooded_pct=100 * truth.mean(), auc=roc_auc_score(truth, score),
               avg_precision=average_precision_score(truth, score), baseline_precision=truth.mean())
    for s in (0.05, 0.10):
        row[f"precision_top{int(s * 100)}"], row[f"recall_top{int(s * 100)}"] = top_share(score, truth, s)
    return row


def risk_eval(rivers, features=FEATURES):
    df, grids = pd.read_pickle(CACHE / f"risk_features_{rivers}.pkl")
    rows = []
    for name, g in grids.items():
        truth = np.asarray(g["flooded"], bool)
        if truth.sum() < 10 or (~truth).sum() < 10:
            continue
        model = new_model().fit(df.loc[df.area != name, features], df.loc[df.area != name, "flooded"])
        rows.append(dict(model=f"risk ({rivers} rivers)", **grid_scores(name, model.predict_proba(g["feats"][features])[:, 1], truth)))
        rows.append(dict(model="EFAS RP100 depth", **grid_scores(name, g["feats"]["rp100"].to_numpy(), truth)))
    return pd.DataFrame(rows)


def damage_eval():
    """CEMS builtUpP only lists damaged buildings, so two tests:
    (a) hit: graded buildings vs GHS-OBAT buildings within 1 km of a graded one but > 20 m from any;
    (b) severity: damaged / destroyed vs possibly damaged, among graded buildings."""
    from access_model import buildings_in
    from scipy.spatial import cKDTree
    rows = []
    for folder in sorted(DATA.glob("ems_*/*_GRA_PRODUCT_*_vector")):
        path = next(iter(folder.glob("*_builtUpP_*.shp")), None)
        if path is None:
            continue
        g = gpd.read_file(path).to_crs(3035)
        g = g[g.damage_gra.isin(DAMAGED)]
        area = Area(folder)
        gxy = shapely.get_coordinates(g.geometry.representative_point().values)
        _, oxy = buildings_in(area.aoi)
        d = cKDTree(gxy).query(oxy)[0]
        nxy = oxy[(d > 20) & (d <= 1000)]
        tif = risk_model.OUT / f"risk_{area.name}.tif"

        def scores(xy):
            out = {"EFAS RP100 depth (DEM-checked)": area.depth("rp100", xy),
                   "EFAS RP100 depth (raw)": area.depth("rp100", xy, dem_check=False),
                   "inside observed 2021 extent": shapely.contains_xy(area.flood, xy[:, 0], xy[:, 1]).astype(float)}
            if tif.exists():
                out["terrain risk (held-out map)"] = np.nan_to_num(sample_raster(tif, xy))
            return out

        tests = {"hit": (np.vstack([gxy, nxy]), np.r_[np.ones(len(gxy)), np.zeros(len(nxy))].astype(bool)),
                 "severity": (gxy, g.damage_gra.isin({"Damaged", "Destroyed"}).to_numpy())}
        for test, (xy, y) in tests.items():
            if y.sum() < 5 or (~y).sum() < 5:
                continue
            for k, sc in scores(xy).items():
                rows.append(dict(test=test, area=area.name, score=k, n=len(y), positive_pct=100 * y.mean(), auc=roc_auc_score(y, sc)))
        print(f"{area.name}: {len(gxy)} graded, {len(nxy)} ungraded nearby", flush=True)
        del area
    return pd.DataFrame(rows)


if __name__ == "__main__":
    p = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    p.parse_args()
    OUT.mkdir(parents=True, exist_ok=True)
    pd.set_option("display.width", 200)

    risk = pd.concat([risk_eval(r) for r in ("osm", "cems") if (CACHE / f"risk_features_{r}.pkl").exists()])
    risk = risk.drop_duplicates(["model", "area"])
    risk.round(4).to_csv(OUT / "risk_grid.csv", index=False)
    med = risk.groupby("model")[["auc", "avg_precision", "baseline_precision", "precision_top5", "recall_top5",
                                 "precision_top10", "recall_top10"]].median()
    print("\n1. Flood extent on the full held-out AOI grid (medians over AOIs)\n" + med.round(3).to_string())

    dmg = damage_eval()
    dmg.round(3).to_csv(OUT / "building_damage.csv", index=False)
    print("\n2. Building damage, ROC AUC (hit: graded vs ungraded nearby; severity: damaged/destroyed vs possibly)\n"
          + dmg.pivot_table(index=["test", "area"], columns="score", values="auc").round(3).to_string())

    summary = [dict(model=m, check="flood extent, held-out AOI grid", metric="median ROC AUC / avg precision",
                    value=f"{r.auc:.3f} / {r.avg_precision:.3f} (base rate {r.baseline_precision:.3f})") for m, r in med.iterrows()]
    summary += [dict(model=sc, check=f"building damage, {t} (CEMS grades)", metric="median ROC AUC", value=f"{v:.3f}")
                for (t, sc), v in dmg.groupby(["test", "score"]).auc.median().items()]
    for f, check in [("roads/damage_validation.csv", "road damage (CEMS grades)"), ("eval/depth_check.csv", "DEM depth check")]:
        if (OUTPUTS / f).exists():
            summary.append(dict(model=f.split("/")[0], check=check, metric="see file", value=str(OUTPUTS / f)))
    summary.append(dict(model="interp_model", check="predict AOI15 18/07 from 16 + 20/07", metric="IoU",
                        value="0.14 (shape morphing 0.007, nearest image 0.02)"))
    pd.DataFrame(summary).to_csv(OUT / "summary.csv", index=False)
    print(f"\nWrote {OUT}/summary.csv, risk_grid.csv, building_damage.csv")
