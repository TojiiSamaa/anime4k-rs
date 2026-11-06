#!/usr/bin/env python3
"""Render bench results: SVG card (README) + Markdown table (job summary / PR comment).

Usage: render.py result.json [--history DIR] [--svg bench.svg] [--md summary.md]
"""

import argparse, json
from pathlib import Path
from xml.sax.saxutils import escape

COLORS = {
    "ours": "var(--ours)",
    "theirs": "var(--theirs)",
    "reference": "var(--ref)",
    "baseline": "var(--base)",
}
KIND = {
    "ours": "this repo",
    "theirs": "Rust port",
    "reference": "reference",
    "baseline": "baseline",
}


def fmt(v, unit="", nd=2):
    return "—" if v is None else f"{v:.{nd}f}{unit}"


def reliability(e):
    if e.get("error") and not e.get("runs_ok"):
        return "build/run failed"
    ok = f"{e['runs_ok']}/{e['runs']} runs ok"
    return ok + (
        " · deterministic" if e.get("deterministic") else " · NON-deterministic"
    )


def markdown(r, base=None):
    out = [
        f"### Benchmark — { {'clean': 'clean source', 'degraded': 'blurred + H.264 source'}.get(r.get('source'), 'clean source') }, {r['input']} → {r['output']}, preset {r['preset']}, {r['frames']} images",
        f"`{r['machine']['cpu']}` · {r['machine']['threads']} threads · commit `{r['commit']}` · {r['date']}",
        "",
        "| Implementation | Version | ms / image | PSNR vs ground truth | SSIM | Fidelity vs GLSL | Reliability |",
        "|---|---|---:|---:|---:|---:|---|",
    ]
    for e in r["entries"]:
        out.append(
            f"| {e['impl']} | `{e['version']}` | {fmt(e.get('ms'), '', 0)} | {fmt(e.get('psnr'), ' dB')} | "
            f"{fmt(e.get('ssim'), '', 4)} | {fmt(e.get('fidelity_psnr'), ' dB')} | {reliability(e)} |"
        )
    ours = [e for e in r["entries"] if e["kind"] == "ours" and e.get("ms")]
    if len(ours) >= 2:
        a, b = ours[0], ours[-1]
        out += [
            "",
            f"**{b['version']} vs {a['version']}** : time {pct(a['ms'], b['ms'])}, "
            f"PSNR {b['psnr'] - a['psnr']:+.3f} dB, SSIM {b['ssim'] - a['ssim']:+.4f}",
        ]
    out += [
        "",
        "_Ground truth: 1080p crops of Pepper&Carrot (David Revoy, CC BY 4.0) downscaled 2× then upscaled back. PSNR/SSIM reward closeness, not perceived sharpness. "
        "Fidelity = PSNR between our native engine and the original shaders run by libplacebo (higher = closer)._",
    ]
    return "\n".join(out)


def pct(a, b):
    return f"{(b - a) / a * 100:+.1f}%" + (
        " (faster)" if b < a else " (slower)" if b > a else ""
    )


def history(d):
    pts = []
    for f in sorted(Path(d).glob("*.json")):
        r = json.loads(f.read_text())
        e = next((e for e in r["entries"] if e["kind"] == "ours" and e.get("ms")), None)
        if e:
            pts.append((r["date"], e["version"], e["ms"], e["psnr"]))
    return sorted(pts)[-12:]


def svg(r, hist):
    # one reference row is enough: the GLSL shaders do not change between our versions
    refs = [e for e in r["entries"] if e["kind"] == "reference"]
    rows = [e for e in r["entries"] if e["kind"] != "reference" or e is refs[-1]]
    W, top, rh = 900, 104, 46
    H = top + rh * len(rows) + (150 if len(hist) >= 2 else 20) + 44
    ms_max = max((e.get("ms") or 0) for e in rows) or 1
    x_name, x_bar, bar_w, x_cols = 24, 300, 180, 560
    s = [
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{H}" viewBox="0 0 {W} {H}" '
        f'font-family="Inter,Segoe UI,Helvetica,Arial,sans-serif">',
        """<style>
:root{--bg:#ffffff;--fg:#1f2328;--mut:#656d76;--line:#d0d7de;--ours:#e8590c;--theirs:#1f6feb;--ref:#8250df;--base:#8c959f;--ok:#1a7f37;--bad:#cf222e}
@media (prefers-color-scheme:dark){:root{--bg:#0d1117;--fg:#e6edf3;--mut:#8d96a0;--line:#30363d;--ours:#ff8a3d;--theirs:#58a6ff;--ref:#bc8cff;--base:#6e7681;--ok:#3fb950;--bad:#f85149}}
.t{fill:var(--fg)}.m{fill:var(--mut)}.b{font-weight:600}
</style>""",
        f'<rect width="{W}" height="{H}" rx="12" fill="var(--bg)" stroke="var(--line)"/>',
        f'<text x="{x_name}" y="40" class="t b" font-size="22">anime4k-rs benchmark · { {"clean": "clean source", "degraded": "blurred + H.264 source"}.get(r.get("source"), "clean source") }</text>',
        f'<text x="{x_name}" y="64" class="m" font-size="13">{escape(r["input"])} → {escape(r["output"])} · preset {escape(r["preset"])} · '
        f"{r['frames']} images · {escape(r['machine']['cpu'])} ({r['machine']['threads']} threads) · {escape(r['date'][:10])} · {escape(r['commit'])}</text>",
    ]
    hy = top - 12
    for x, t in (
        (x_name, "Implementation"),
        (x_bar, "Time per image"),
        (x_cols, "PSNR"),
        (x_cols + 80, "SSIM"),
        (x_cols + 150, "Fidelity"),
        (x_cols + 230, "Reliability"),
    ):
        s.append(
            f'<text x="{x}" y="{hy}" class="m" font-size="11" letter-spacing=".04em">{t.upper()}</text>'
        )
    best = max((e.get("psnr") or 0) for e in rows)
    for i, e in enumerate(rows):
        y = top + i * rh
        c = COLORS.get(e["kind"], "var(--base)")
        s.append(
            f'<line x1="{x_name}" x2="{W - 24}" y1="{y}" y2="{y}" stroke="var(--line)"/>'
        )
        s.append(
            f'<rect x="{x_name}" y="{y + 14}" width="4" height="20" rx="2" fill="{c}"/>'
        )
        s.append(
            f'<text x="{x_name + 14}" y="{y + 24}" class="t b" font-size="14">{escape(e["impl"])}</text>'
        )
        s.append(
            f'<text x="{x_name + 14}" y="{y + 40}" class="m" font-size="11">{KIND.get(e["kind"], "")} · {escape(e["version"])}</text>'
        )
        if e.get("ms"):
            w = max(3, bar_w * e["ms"] / ms_max)
            s.append(
                f'<rect x="{x_bar}" y="{y + 15}" width="{w:.1f}" height="16" rx="4" fill="{c}" opacity=".9"/>'
            )
            s.append(
                f'<text x="{x_bar + w + 6:.1f}" y="{y + 28}" class="t" font-size="12">{ms_label(e["ms"])}</text>'
            )
        p = e.get("psnr")
        s.append(
            f'<text x="{x_cols}" y="{y + 28}" class="t{" b" if p == best else ""}" font-size="13">{fmt(p, " dB")}</text>'
        )
        s.append(
            f'<text x="{x_cols + 80}" y="{y + 28}" class="t" font-size="13">{fmt(e.get("ssim"), "", 4)}</text>'
        )
        s.append(
            f'<text x="{x_cols + 150}" y="{y + 28}" class="t" font-size="13">{fmt(e.get("fidelity_psnr"), " dB", 1)}</text>'
        )
        good = e.get("runs_ok") and e["runs_ok"] == e["runs"] and e.get("deterministic")
        rel = (
            (
                f"{e['runs_ok']}/{e['runs']} ok"
                + (" · det." if e.get("deterministic") else " · non-det.")
            )
            if e.get("runs")
            else "failed"
        )
        s.append(
            f'<text x="{x_cols + 230}" y="{y + 28}" font-size="13" fill="var({"--ok" if good else "--bad"})">{"✓ " if good else "✗ "}{rel}</text>'
        )
    y = top + rh * len(rows)
    s.append(
        f'<line x1="{x_name}" x2="{W - 24}" y1="{y}" y2="{y}" stroke="var(--line)"/>'
    )
    if len(hist) >= 2:
        s += spark(hist, x_name, y + 30, W - 48, 90)
        y += 150
    s.append(
        f'<text x="{x_name}" y="{H - 34}" class="m" font-size="11">Ground truth: 1080p crops of Pepper&amp;Carrot (D. Revoy, CC BY 4.0), '
        f"downscaled 2× then upscaled. PSNR/SSIM measure closeness, not sharpness.</text>"
        f'<text x="{x_name}" y="{H - 16}" class="m" font-size="11">Fidelity = PSNR of our native engine vs the original GLSL shaders run by libplacebo. '
        f"Reliability = runs ok, and bit-identical output on two runs.</text>"
    )
    s.append("</svg>")
    return "\n".join(s)


def ms_label(ms):
    return f"{ms / 1000:.2f} s" if ms >= 1000 else f"{ms:.0f} ms"


def spark(hist, x, y, w, h):
    """Trend of our native engine across versions: time (bars) and PSNR (line)."""
    s = [
        f'<text x="{x}" y="{y - 8}" class="m" font-size="11" letter-spacing=".04em">HISTORY · NATIVE ENGINE · TIME (BARS) AND PSNR (LINE)</text>'
    ]
    n = len(hist)
    step = w / n
    mmax = max(p[2] for p in hist)
    pmin, pmax = min(p[3] for p in hist), max(p[3] for p in hist)
    pts = []
    for i, (_, v, ms, ps) in enumerate(hist):
        cx = x + step * i + step / 2
        bh = (h - 30) * ms / mmax
        s.append(
            f'<rect x="{cx - step * 0.3:.1f}" y="{y + h - 20 - bh:.1f}" width="{step * 0.6:.1f}" height="{bh:.1f}" rx="3" fill="var(--ours)" opacity=".35"/>'
        )
        s.append(
            f'<text x="{cx:.1f}" y="{y + h - 6}" class="m" font-size="10" text-anchor="middle">{escape(v[:14])}</text>'
        )
        py = (
            y
            + 6
            + (h - 40) * (1 - ((ps - pmin) / (pmax - pmin) if pmax > pmin else 0.5))
        )
        pts.append(f"{cx:.1f},{py:.1f}")
        s.append(
            f'<text x="{cx:.1f}" y="{py - 6:.1f}" class="t" font-size="10" text-anchor="middle">{ps:.2f}</text>'
        )
    s.append(
        f'<polyline points="{" ".join(pts)}" fill="none" stroke="var(--ours)" stroke-width="2"/>'
    )
    return s


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("result")
    ap.add_argument("--history")
    ap.add_argument("--svg")
    ap.add_argument("--md")
    a = ap.parse_args()
    r = json.loads(Path(a.result).read_text())
    if a.svg:
        Path(a.svg).write_text(
            svg(r, history(a.history) if a.history else []), encoding="utf-8"
        )
    md = markdown(r)
    if a.md:
        Path(a.md).write_text(md, encoding="utf-8")
    else:
        print(md)


if __name__ == "__main__":
    main()
