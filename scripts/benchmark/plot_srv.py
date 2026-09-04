#!/usr/bin/env python3
"""Render the 1 GB server benchmark: every tool, four metrics.

Reads `srv.csv` (app,config,pass,seconds,bytes,cpu_seconds,max_rss_kb,checksum).
Rows whose run failed carry a non-numeric `seconds` and are dropped rather than
guessed at. Bars are medians; the duration whisker is the run-to-run range.
"""
import csv, os, statistics, collections
import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt

SIZE = 1073741824
HERE = os.path.dirname(os.path.abspath(__file__))
rows = []
for r in csv.DictReader(open(os.path.join(HERE, "srv.csv"))):
    if not r.get("app"):
        continue
    try:
        float(r["seconds"])
    except ValueError:
        continue
    rows.append(r)

g = collections.defaultdict(list)
for r in rows:
    g[(r["app"], r["config"])].append(r)

stats = []
for (app, cfg), v in g.items():
    s = [float(x["seconds"]) for x in v]
    med = statistics.median(s)
    stats.append({
        "app": app, "label": f"{app} {cfg}",
        "secs": med, "lo": min(s), "hi": max(s),
        "mbs": SIZE / med / 1e6,
        "rss": statistics.median(float(x["max_rss_kb"]) for x in v) / 1024,
        "cpu": statistics.median(float(x["cpu_seconds"]) for x in v),
        "n": len(v),
    })

# axel buffers the whole object in memory (977 MiB peak) and lands an order of
# magnitude off every other tool. Keeping it in the chart would flatten the
# comparison everyone actually wants to read, so it stays in the table only.
plotted = [d for d in stats if d["app"] != "axel"]
plotted.sort(key=lambda d: d["mbs"])

def colour(app):
    return {"hydra": "#2f7ed8"}.get(app, "#9aa5b1")

colours = [colour(d["app"]) for d in plotted]
labels = [d["label"] for d in plotted]
y = range(len(plotted))

fig, axes = plt.subplots(2, 2, figsize=(13, 8.5), facecolor="white")
fig.suptitle(
    "1 GB from ash-speed.hetzner.com — Hetzner VPS, 97 ms RTT, median of 4 runs each",
    fontsize=13, fontweight="bold", y=0.985,
)
for ax, key, title, sub, fmt in [
    (axes[0][0], "mbs", "Average speed (MB/s)", "Higher is better", "{:.1f}"),
    (axes[0][1], "secs", "Time to complete (s)", "Lower is better, whisker is the run-to-run range", "{:.1f}"),
    (axes[1][0], "rss", "Peak memory (MiB)", "Lower is better", "{:.1f}"),
    (axes[1][1], "cpu", "CPU time (s)", "Lower is better", "{:.1f}"),
]:
    vals = [d[key] for d in plotted]
    err = None
    if key == "secs":
        err = [[d["secs"] - d["lo"] for d in plotted], [d["hi"] - d["secs"] for d in plotted]]
    ax.barh(list(y), vals, color=colours, height=0.66, xerr=err,
            error_kw={"ecolor": "#5b6470", "elinewidth": 1.0, "capsize": 2.5})
    ax.set_yticks(list(y)); ax.set_yticklabels(labels, fontsize=9.5)
    ax.set_title(f"{title}\n{sub}", fontsize=10.5, loc="left")
    ax.grid(axis="x", color="#e6e8eb", linewidth=0.8); ax.set_axisbelow(True)
    for sp in ("top", "right", "left"):
        ax.spines[sp].set_visible(False)
    span = max(d["hi"] for d in plotted) if key == "secs" else max(vals)
    for i, (d, v) in enumerate(zip(plotted, vals)):
        at = d["hi"] if key == "secs" else v
        ax.text(at + span * 0.018, i, fmt.format(v), va="center", fontsize=8.5, color="#333")
    ax.set_xlim(0, span * 1.2)

fig.text(0.5, 0.012,
         "One process at a time, order reversed on alternate passes. axel omitted from the chart: "
         "5.5 MB/s at 977 MiB peak memory, an order of magnitude off the rest.",
         ha="center", fontsize=8.5, color="#666")
fig.tight_layout(rect=(0, 0.03, 1, 0.96))
for out in ("srv-1gb.png", "srv-1gb.svg"):
    fig.savefig(os.path.join(HERE, out), dpi=160, facecolor="white")

stats.sort(key=lambda d: -d["mbs"])
lines = ["| Application | Config | Avg speed | Time to complete | Range over runs | Peak memory | CPU time |",
         "| --- | --- | --- | --- | --- | --- | --- |"]
for d in stats:
    app, _, cfg = d["label"].partition(" ")
    lines.append(f"| {app} | `{cfg}` | **{d['mbs']:.1f} MB/s** | {d['secs']:.1f} s | "
                 f"{d['lo']:.1f}-{d['hi']:.1f} s | {d['rss']:.0f} MiB | {d['cpu']:.1f} s |")
open(os.path.join(HERE, "srv-table.md"), "w").write("\n".join(lines) + "\n")
print("\n".join(lines))
