#!/usr/bin/env python3
"""Render the deterministic-bed comparison: every client, four metrics.

Reads `bed.csv` (app,config,mb_per_s,cpu_seconds,max_rss_mib), each row the
mean of three runs on the VPS test bed: nginx serving 1 GB from RAM inside a
network namespace, 100 ms of netem round trip, one process at a time.
"""
import csv, os
import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt

SIZE = 1073741824
HERE = os.path.dirname(os.path.abspath(__file__))
rows = [r for r in csv.DictReader(open(os.path.join(HERE, "bed.csv"))) if r.get("app")]
for r in rows:
    r["mbs"] = float(r["mb_per_s"]); r["cpu"] = float(r["cpu_seconds"])
    r["rss"] = float(r["max_rss_mib"]); r["secs"] = SIZE / r["mbs"] / 1e6
    r["label"] = f'{r["app"]} {r["config"]}'
rows.sort(key=lambda r: r["mbs"])

def colour(r):
    if r["app"] != "hydra":
        return "#9aa5b1"
    return "#1a5fb4" if r["config"] == "default" else "#7fb3e8"

labels = [r["label"] for r in rows]; colours = [colour(r) for r in rows]; y = range(len(rows))
fig, axes = plt.subplots(2, 2, figsize=(13, 8.5), facecolor="white")
fig.suptitle("1 GB over a 100 ms path — VPS test bed, mean of 3 runs, one client at a time",
             fontsize=13, fontweight="bold", y=0.985)
for ax, key, title, sub, fmt in [
    (axes[0][0], "mbs", "Average speed (MB/s)", "Higher is better", "{:.0f}"),
    (axes[0][1], "secs", "Time to complete (s)", "Lower is better", "{:.1f}"),
    (axes[1][0], "rss", "Peak memory (MiB)", "Lower is better", "{:.1f}"),
    (axes[1][1], "cpu", "CPU time (s)", "Lower is better", "{:.2f}"),
]:
    vals = [r[key] for r in rows]
    ax.barh(list(y), vals, color=colours, height=0.66)
    ax.set_yticks(list(y)); ax.set_yticklabels(labels, fontsize=9.5)
    ax.set_title(f"{title}\n{sub}", fontsize=10.5, loc="left")
    ax.grid(axis="x", color="#e6e8eb", linewidth=0.8); ax.set_axisbelow(True)
    for sp in ("top", "right", "left"):
        ax.spines[sp].set_visible(False)
    span = max(vals)
    for i, v in zip(y, vals):
        ax.text(v + span * 0.018, i, fmt.format(v), va="center", fontsize=8.5, color="#333")
    ax.set_xlim(0, span * 1.18)
fig.text(0.5, 0.012,
         "Dark blue is a bare `hydra <url>` with no flags. nginx serves from RAM in a network namespace with "
         "tc netem 50 ms each way; four plain curl ranges finish within 10 ms of each other, so the path is fair.",
         ha="center", fontsize=8.5, color="#666")
fig.tight_layout(rect=(0, 0.03, 1, 0.96))
for out in ("bed.png", "bed.svg"):
    fig.savefig(os.path.join(HERE, out), dpi=160, facecolor="white")
rows.sort(key=lambda r: -r["mbs"])
lines = ["| Application | Config | Avg speed | Time to complete | Peak memory | CPU time |",
         "| --- | --- | --- | --- | --- | --- |"]
for r in rows:
    lines.append(f'| {r["app"]} | `{r["config"]}` | **{r["mbs"]:.0f} MB/s** | {r["secs"]:.1f} s | '
                 f'{r["rss"]:.1f} MiB | {r["cpu"]:.2f} s |')
open(os.path.join(HERE, "bed-table.md"), "w").write("\n".join(lines) + "\n")
print("\n".join(lines))
