"""Run the impact pipeline over every CEMS PRODUCT AOI and write a PDF summary."""
import argparse, contextlib, datetime, io, re
import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt
import pandas as pd
from pathlib import Path
from reportlab.lib import colors
from reportlab.lib.pagesizes import A4
from reportlab.lib.styles import getSampleStyleSheet, ParagraphStyle
from reportlab.lib.units import mm
from reportlab.platypus import SimpleDocTemplate, Paragraph, Spacer, Table, TableStyle, Image, PageBreak, KeepTogether

from main import DATA, flood_file, guess_buildings
from paths import OUTPUTS
import pipeline

COUNTRY = {"be": "Belgium", "de": "Germany", "nl": "Netherlands"}
BLUE, INK, MUTED, RULE = colors.HexColor("#1f5f8b"), colors.HexColor("#1b1f24"), colors.HexColor("#5b6670"), colors.HexColor("#d5dbe1")


def collect(folders, cost, max_depth, map_dir):
    rows = []
    for folder in folders:
        path = flood_file(folder)
        m = re.match(r"(EMSR\d+)_(AOI\d+)_(DEL|GRA)", folder.name)
        row = dict(country=COUNTRY[folder.parent.name[4:]], activation=m[1], aoi=m[2], product=m[3])
        print(f"{folder.parent.name}/{m[1]} {m[2]} {m[3]}", flush=True)
        bld_path = guess_buildings(path)
        with contextlib.redirect_stdout(io.StringIO()):
            s = pipeline.run(path, bld_path, cost_m2=cost, max_depth=max_depth)
        flood, bld = s["flood"], s["buildings"]
        hit = bld[bld.flooded]
        row.update(
            polygons=len(flood), area_km2=flood.to_crs(3035).area.sum() / 1e6,
            flooded=len(hit), depth=hit.depth.mean() if len(hit) else 0, capped=(hit.depth >= max_depth).sum(),
            damage=hit.damage_frac.mean() if len(hit) else 0, loss_m=hit.loss_eur.sum() / 1e6,
            affected=hit.occupants.sum(), displaced=bld.loc[bld.depth >= pipeline.DISPLACED_M, "occupants"].sum(),
        )
        row["map"] = draw_map(flood, hit, map_dir / f"{m[1]}_{m[2]}_{m[3]}.png")
        rows.append(row)
    return pd.DataFrame(rows)


def draw_map(flood, hit, out):
    fig, ax = plt.subplots(figsize=(4.2, 3.4), dpi=170)
    flood.plot(ax=ax, color="#9ecae1", edgecolor="#4a90c2", linewidth=0.3)
    if len(hit):
        sc = ax.scatter(hit.geometry.x, hit.geometry.y, c=hit.damage_frac, cmap="OrRd", vmin=0, vmax=1, s=6,
                        edgecolors="#333", linewidths=0.2, zorder=3)
        cb = fig.colorbar(sc, ax=ax, fraction=0.04, pad=0.02)
        cb.set_label("damage fraction", fontsize=7); cb.ax.tick_params(labelsize=6)
    ax.set_aspect(1 / 0.64)  # roughly equal-distance at ~50°N
    ax.tick_params(labelsize=6); ax.grid(color="#e5e5e5", linewidth=0.4)
    for sp in ax.spines.values():
        sp.set_color("#bbb")
    fig.tight_layout(); fig.savefig(out); plt.close(fig)
    return out


def table(data, widths, num_cols=(), total=False):
    t = Table(data, colWidths=widths, repeatRows=1)
    style = [
        ("FONT", (0, 0), (-1, 0), "Helvetica-Bold", 7.5), ("FONT", (0, 1), (-1, -1), "Helvetica", 7.5),
        ("TEXTCOLOR", (0, 0), (-1, 0), colors.white), ("BACKGROUND", (0, 0), (-1, 0), BLUE),
        ("ROWBACKGROUNDS", (0, 1), (-1, -1), [colors.white, colors.HexColor("#f3f6f9")]),
        ("LINEBELOW", (0, 0), (-1, -1), 0.25, RULE), ("VALIGN", (0, 0), (-1, -1), "MIDDLE"),
        ("TOPPADDING", (0, 0), (-1, -1), 3), ("BOTTOMPADDING", (0, 0), (-1, -1), 3),
    ] + [("ALIGN", (c, 0), (c, -1), "RIGHT") for c in num_cols]
    if total:
        style += [("FONT", (0, -1), (-1, -1), "Helvetica-Bold", 7.5), ("LINEABOVE", (0, -1), (-1, -1), 0.8, INK),
                  ("BACKGROUND", (0, -1), (-1, -1), colors.white)]
    t.setStyle(TableStyle(style))
    return t


def build_pdf(df, out, cost, max_depth):
    ss = getSampleStyleSheet()
    H1 = ParagraphStyle("H1", parent=ss["Title"], fontSize=20, leading=24, alignment=0, textColor=INK, spaceAfter=2)
    H2 = ParagraphStyle("H2", parent=ss["Heading2"], fontSize=12.5, textColor=BLUE, spaceBefore=10, spaceAfter=5)
    P = ParagraphStyle("P", parent=ss["BodyText"], fontSize=8.8, leading=12, textColor=INK)
    SMALL = ParagraphStyle("S", parent=P, fontSize=7.5, leading=10, textColor=MUTED)
    W = A4[0] - 36 * mm
    story = [
        Paragraph("Flood Impact Summary", H1),
        Paragraph(f"July 2021 floods: Belgium, Germany, Netherlands · CEMS activations "
                  f"{', '.join(sorted(df.activation.unique()))} · generated {datetime.date.today():%d %B %Y}", SMALL),
        Spacer(1, 10),
    ]

    # Headline figures
    kpis = [("Flood extent", f"{df.area_km2.sum():.1f} km²"), ("Buildings flooded", f"{df.flooded.sum():,}"),
            ("Direct building damage", f"€{df.loss_m.sum():,.0f} M"), ("People displaced", f"~{df.displaced.sum():,.0f}")]
    kpi = Table([[Paragraph(f'<font size="7.5" color="#5b6670">{k}</font><br/><font size="15"><b>{v}</b></font>', P)
                  for k, v in kpis]], colWidths=[W / 4] * 4)
    kpi.setStyle(TableStyle([("BOX", (0, 0), (-1, -1), 0.5, RULE), ("INNERGRID", (0, 0), (-1, -1), 0.5, RULE),
                             ("TOPPADDING", (0, 0), (-1, -1), 7), ("BOTTOMPADDING", (0, 0), (-1, -1), 7),
                             ("BACKGROUND", (0, 0), (-1, -1), colors.HexColor("#f3f6f9"))]))
    story += [kpi, Spacer(1, 4),
              Paragraph(f"{len(df)} areas of interest, one initial (PRODUCT) map each; monitoring updates excluded to "
                        "avoid double counting. Figures are model estimates. See Method and caveats.", SMALL)]

    # By country
    story.append(Paragraph("By country", H2))
    g = df.groupby("country").agg(aois=("aoi", "size"), area=("area_km2", "sum"), flooded=("flooded", "sum"),
                                  loss=("loss_m", "sum"), displaced=("displaced", "sum")).reset_index()
    rows = [["Country", "AOIs", "Flood extent (km²)", "Buildings flooded", "Damage (€ M)", "People displaced"]]
    rows += [[r.country, r.aois, f"{r.area:.2f}", f"{r.flooded:,}", f"{r.loss:,.1f}", f"{r.displaced:,.0f}"] for r in g.itertuples()]
    rows.append(["Total", g.aois.sum(), f"{g.area.sum():.2f}", f"{g.flooded.sum():,}", f"{g.loss.sum():,.1f}", f"{g.displaced.sum():,.0f}"])
    story.append(table(rows, [W * f for f in (.22, .1, .17, .17, .17, .17)], num_cols=range(1, 6), total=True))

    # Per AOI
    story.append(Paragraph("By area of interest", H2))
    rows = [["Country", "Activation", "AOI", "Type", "Extent\n(km²)", "Buildings\nflooded", "Mean\ndepth (m)",
             "Mean\ndamage", "Damage\n(€ M)", "Displaced"]]
    for r in df.sort_values(["country", "aoi"]).itertuples():
        rows.append([r.country, r.activation, r.aoi, r.product, f"{r.area_km2:.2f}", f"{r.flooded:,}",
                     f"{r.depth:.2f}" + ("*" if r.capped and r.capped == r.flooded else ""),
                     f"{r.damage:.0%}", f"{r.loss_m:,.2f}", f"{r.displaced:,.0f}"])
    story.append(table(rows, [W * f for f in (.13, .11, .08, .07, .1, .11, .1, .09, .11, .1)], num_cols=range(4, 10)))
    story.append(Spacer(1, 3))
    story.append(Paragraph(f"* every flooded building hit the {max_depth:g} m depth cap: the RP100 depth map overshoots "
                           "here, so damage is saturated. Type: DEL = flood delineation, GRA = damage grading map.", SMALL))

    # Method
    story.append(Paragraph("Method and caveats", H2))
    for line in [
        "<b>Hazard</b>: observed flood extent from the CEMS Rapid Mapping <i>observedEventA</i> layer.",
        "<b>Exposure</b>: GHS-OBAT building points that fall inside the extent. Depth at each building is sampled "
        f"from the CEMS-EFAS 1-in-100-year depth map (capped at {max_depth:g} m). This is a scenario depth, "
        "not the observed depth of this event, and it is the weakest link in the chain.",
        "<b>Vulnerability</b>: JRC Europe depth-damage curves (Huizinga et al. 2017), residential or commercial "
        "by OBAT use class (assumed codes: 1 residential, 2 non-residential, 0 unknown).",
        f"<b>Loss</b>: damage fraction × floor area (footprint × storeys at 3 m each) × €{cost:,.0f}/m², a placeholder replacement cost.",
        "<b>People</b>: occupants = residential floor area / 40 m²; displaced where depth ≥ 0.5 m.",
        "Buildings are counted only if strictly inside a flood polygon. CEMS extents often exclude built-up areas "
        "next to the river, so building and people counts are likely <b>underestimates</b>.",
    ]:
        story.append(Paragraph("• " + line, P))

    # Maps, two per row
    story += [PageBreak(), Paragraph("Maps", H2),
              Paragraph("Blue: observed flood extent. Dots: flooded buildings coloured by modelled damage fraction.", SMALL),
              Spacer(1, 6)]
    cells = []
    for r in df.sort_values(["country", "aoi"]).itertuples():
        cap = Paragraph(f"<b>{r.country} · {r.activation} {r.aoi} {r.product}</b><br/>"
                        f"{r.area_km2:.2f} km² · {r.flooded:,} buildings · €{r.loss_m:,.2f} M", SMALL)
        cells.append([Image(str(r.map), width=W / 2 - 6, height=(W / 2 - 6) * 3.4 / 4.2), cap])
    grid = [[cells[i], cells[i + 1] if i + 1 < len(cells) else ""] for i in range(0, len(cells), 2)]
    for row in grid:
        t = Table([row], colWidths=[W / 2] * 2)
        t.setStyle(TableStyle([("VALIGN", (0, 0), (-1, -1), "TOP")]))
        story += [KeepTogether(t), Spacer(1, 6)]

    def footer(c, d):
        c.setFont("Helvetica", 7); c.setFillColor(MUTED)
        c.drawString(18 * mm, 10 * mm, "Flood impact summary · CEMS Rapid Mapping, GHS-OBAT, CEMS-EFAS")
        c.drawRightString(A4[0] - 18 * mm, 10 * mm, f"Page {d.page}")

    SimpleDocTemplate(str(out), pagesize=A4, leftMargin=18 * mm, rightMargin=18 * mm, topMargin=16 * mm,
                      bottomMargin=16 * mm, title="Flood Impact Summary").build(story, onFirstPage=footer, onLaterPages=footer)


if __name__ == "__main__":
    p = argparse.ArgumentParser(description="PDF summary of the flood impact pipeline over all CEMS PRODUCT AOIs.")
    p.add_argument("-o", "--out", type=Path, default=OUTPUTS / "report" / "flood_summary.pdf")
    p.add_argument("--cost", type=float, default=1500.0, help="replacement cost, €/m² floor area")
    p.add_argument("--max-depth", type=float, default=6.0, help="cap on sampled depth, m")
    args = p.parse_args()

    folders = sorted(f for d in sorted(DATA.glob("ems_*")) for f in d.glob("*_PRODUCT_*_vector"))
    map_dir = args.out.parent / "maps"; map_dir.mkdir(parents=True, exist_ok=True)
    df = collect(folders, args.cost, args.max_depth, map_dir)
    df.drop(columns="map").to_csv(args.out.with_suffix(".csv"), index=False)
    build_pdf(df, args.out, args.cost, args.max_depth)
    print("wrote", args.out)
