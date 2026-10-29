# Modelling plan

## Key idea: labels vs features
AOIs differ in which EMS layers they have, but that doesn't block a general model:
- **Labels come from EMS** (varies per AOI). Every AOI has `observedEventA`, so "is this building/cell inside `flood_extent`" is a consistent label everywhere.
- **Features come from uniform datasets** with the same columns everywhere:
  - OBAT
  - EFAS
  - GHS-POP
  - distance to river
  - optionally an external DEM / rainfall / soil

## What kind of "simulator"
- The data covers **one event**. ML can learn *where water went in 2021*, not *how flooding changes with more rain*.
- A real physics simulator is LISFLOOD; EFAS already ran it, which is where the RP10/100/500 depth maps come from.
- **Hybrid design:**

```
Scenario (RP10 / RP100 / RP500)
   └─► EFAS depth map  (physics: where/how deep)
         └─► ML model  (learned from 2021: what actually floods, how badly buildings are damaged)
               └─► Map + numbers: flooded buildings, damage classes, people affected
```

## Two models
**Model 1: flood occurrence** (all 8 AOIs)
- Rows: OBAT buildings, or grid cells of 50–100 m inside `aoi_boundary`.
- Label: inside `flood_extent` (yes/no).
- Features:
  - EFAS RP10/100/500 depth
  - distance to river (`hydrographyL`)
  - land use
  - OBAT height / use / epoch
  - (external) elevation above river

**Model 2: damage severity** (EMSR517 AOI15 only)
- Rows: `builtUpP` points, plus OBAT buildings with no EMS point within ~10 m, labelled "none".
- Label: none / possibly damaged / damaged / destroyed.
- Features: same as model 1.

## Algorithm
- **Gradient-boosted trees**: sklearn `HistGradientBoostingClassifier`, LightGBM or XGBoost.
  - Best fit for tabular data with mixed column types.
  - Handles missing values.
  - Fast to train.
  - Explainable with SHAP / feature importance, which helps the pitch.
- **Baseline:** logistic regression.
- **Avoid:**
  - CNN / U-Net: the satellite images aren't in the dataset, and 8 AOIs is too little data.
  - Physics-imitating neural networks: one event, no time series.
- scikit-learn is installed in `.venv`.

## Pitfalls
1. **Imbalance:** about 1% of each AOI flooded. Use `class_weight="balanced"` and evaluate with PR-AUC / F1, not accuracy.
2. **Validation:** leave-one-AOI-out (`GroupKFold`, groups = AOI). A random split leaks nearby buildings between train and test.
3. **Leakage:** never use post-event-derived data (`observedEventA`, `damage_gra`) as features.
4. **Duplicates:** use one stage per AOI (MONIT01 or the PRODUCT ∪ MONIT01 union), never both as separate rows.
5. **Mapped area only:** only use rows inside `aoi_boundary`.
6. **Label quality:** DEL labels (Sentinel-1, 10 m) miss urban flooding. GRA labels are more reliable; consider weighting them higher.

## Next step
Build the per-building table for the 8 AOIs (OBAT + EFAS depth + distance to river + `flood_extent` label) and train a first gradient-boosting model with leave-one-AOI-out validation.
