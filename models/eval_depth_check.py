"""Does checking EFAS depth against the 30 m DEM help? Compared on every road piece in the AOIs
against the roads inside the observed 2021 extent (an incomplete reference: it misses water in
towns, so precision is a lower bound for both variants)."""
import numpy as np, pandas as pd, shapely

from paths import CACHE, OUTPUTS
from main import DATA
from risk_model import Area

THRESH = 0.3


def scores(flag, truth, length):
    tp, fp, fn = length[flag & truth].sum(), length[flag & ~truth].sum(), length[~flag & truth].sum()
    prec, rec = tp / max(tp + fp, 1), tp / max(tp + fn, 1)
    return dict(flagged_km=(tp + fp) / 1000, precision=prec, recall=rec, f1=2 * prec * rec / max(prec + rec, 1e-9))


if __name__ == "__main__":
    pieces = pd.DataFrame(pd.read_pickle(CACHE / "road_pieces.pkl"))
    rows = []
    for name, d in pieces.groupby("area"):
        folder = next(f for e in DATA.glob("ems_*") for f in e.glob(f"{name}_PRODUCT_*_vector"))
        area = Area(folder, rivers="cems")
        xy = shapely.get_coordinates(shapely.line_interpolate_point(d.geometry.values, 0.5, normalized=True))
        for rp in ("rp10", "rp100", "rp500"):
            for check in (False, True):
                rows.append(dict(area=name, rp=rp, dem_check=check, length=d.length_m.to_numpy(),
                                 truth=d.observed.to_numpy(bool), flag=area.depth(rp, xy, check) >= THRESH))
        print(name, flush=True)
        del area
    res = []
    for (rp, check), g in pd.DataFrame(rows).groupby(["rp", "dem_check"]):
        res.append(dict(rp=rp, dem_check=check, **scores(np.concatenate(g.flag.tolist()), np.concatenate(g.truth.tolist()),
                                                         np.concatenate(g.length.tolist()))))
    res = pd.DataFrame(res)
    (OUTPUTS / "eval").mkdir(parents=True, exist_ok=True)
    res.round(3).to_csv(OUTPUTS / "eval" / "depth_check.csv", index=False)
    print("\nRoad km flagged vs roads inside the observed 2021 extent")
    print(res.round(3).to_string(index=False))
