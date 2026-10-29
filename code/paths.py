"""Every folder the scripts read from or write to, in one place."""
from pathlib import Path

CODE = Path(__file__).resolve().parent
ROOT = CODE.parent
DATA = ROOT / "working_data"   # challenge data + downloaded DEM / OSM
OUTPUTS = CODE / "outputs"     # results: maps, tables, models, the PDF
CACHE = CODE / "cache"         # recomputable intermediate tables
