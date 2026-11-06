#!/usr/bin/env python3
"""Benchmark anime4k-rs against other implementations on the same frames.

Ground truth: 1920x1080 crops of Pepper&Carrot (David Revoy, CC BY 4.0, 2D
illustration), downscaled 2x (bicubic), upscaled back 2x by every implementation
and compared with the original crop (PSNR / SSIM).  Also measured: time per image, determinism (two
runs, same hash), success rate and, for our native engine, fidelity to the
reference path (libplacebo running the original GLSL shaders).

Usage: bench.py --out result.json [--source clean|degraded] [--ours "HEAD v0.1.0"] [--theirs "v0.2"] [--frames 3]
Stdlib only.  Needs git, cargo, ffmpeg, curl, unzip.
"""

import argparse, hashlib, json, os, platform, re, shutil, statistics, subprocess, sys, time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CACHE = ROOT / "bench" / ".cache"
WORK = CACHE / "work"
PC = "https://www.peppercarrot.com/0_sources/"
# (name, url, y offset of the 1920x1080 crop once scaled to 1920 wide)
SOURCES = [
    (
        "E01P01",
        PC
        + "ep01_Potion-of-Flight/hi-res/en_Pepper-and-Carrot_by-David-Revoy_E01P01.jpg",
        300,
    ),
    (
        "E01P02",
        PC
        + "ep01_Potion-of-Flight/hi-res/en_Pepper-and-Carrot_by-David-Revoy_E01P02.jpg",
        900,
    ),
    (
        "E01P03",
        PC
        + "ep01_Potion-of-Flight/hi-res/en_Pepper-and-Carrot_by-David-Revoy_E01P03.jpg",
        1400,
    ),
    (
        "cover",
        PC
        + "0ther/artworks/hi-res/2016-02-24_vertical-cover_remake_by-David-Revoy.jpg",
        700,
    ),
]
SHADERS_URL = (
    "https://github.com/bloc97/Anime4K/releases/download/v4.0.1/Anime4K_v4.0.zip"
)
THEIRS_REPO = "https://github.com/andraantariksa/Anime4K-rs"
GT_W, GT_H = 1920, 1080


def sh(*cmd, cwd=None, check=True):
    r = subprocess.run([str(c) for c in cmd], cwd=cwd, capture_output=True, text=True)
    if check and r.returncode:
        raise RuntimeError(f"{' '.join(map(str, cmd))}\n{r.stderr[-2000:]}")
    return r


def timed(*cmd, env=None):
    t = time.perf_counter()
    r = subprocess.run([str(c) for c in cmd], env=env, capture_output=True, text=True)
    return r.returncode == 0, (time.perf_counter() - t) * 1000, r.stderr


def metrics(img, gt):
    """(PSNR dB, SSIM) of img vs gt, via ffmpeg."""
    out = []
    for f, rx in (("psnr", r"average:([\d.]+|inf)"), ("ssim", r"All:([\d.]+)")):
        e = sh(
            "ffmpeg",
            "-hide_banner",
            "-i",
            img,
            "-i",
            gt,
            "-lavfi",
            f"[0:v][1:v]{f}",
            "-f",
            "null",
            "-",
        ).stderr
        out.append(float(re.findall(rx, e)[-1]))
    return out


def raw_to_png(raw, png):
    """gbrpf32le output of `--brut` -> 8-bit PNG at ground-truth size (spline, like the encoder)."""
    px = Path(raw).stat().st_size // 12
    s = next((s for s in (1, 2, 4) if (GT_W // 2 * s) * (GT_H // 2 * s) == px), None)
    if s is None:
        raise RuntimeError(f"{raw}: unexpected size")
    sh(
        "ffmpeg",
        "-hide_banner",
        "-loglevel",
        "error",
        "-f",
        "rawvideo",
        "-pix_fmt",
        "gbrpf32le",
        "-s",
        f"{GT_W // 2 * s}x{GT_H // 2 * s}",
        "-i",
        raw,
        "-vf",
        f"scale={GT_W}:{GT_H}:flags=spline,format=rgb24",
        "-y",
        png,
    )


def frames(n, source="clean"):
    CACHE.mkdir(parents=True, exist_ok=True)
    shaders = CACHE / "shaders"
    if not (shaders / "Anime4K_Upscale_CNN_x2_VL.glsl").exists():
        sh("curl", "-fsSL", "-o", CACHE / "shaders.zip", SHADERS_URL)
        sh("unzip", "-qo", CACHE / "shaders.zip", "-d", shaders)
    out = []
    for name, url, y in SOURCES[:n]:
        src, gt, low = (
            CACHE / f"{name}.jpg",
            CACHE / f"gt-{name}.png",
            CACHE / f"low-{source}-{name}.png",
        )
        if not low.exists():
            if not src.exists():
                sh("curl", "-fsSL", "-o", src, url)
            sh(
                "ffmpeg",
                "-hide_banner",
                "-loglevel",
                "error",
                "-i",
                src,
                "-vf",
                f"scale={GT_W}:-1:flags=lanczos,crop={GT_W}:{GT_H}:0:{y},format=rgb24",
                "-y",
                gt,
            )
            sh(
                "ffmpeg",
                "-hide_banner",
                "-loglevel",
                "error",
                "-i",
                gt,
                "-vf",
                f"scale={GT_W // 2}:{GT_H // 2}:flags=bicubic,format=rgb24",
                "-y",
                low,
            )
            if source == "degraded":
                # what Anime4K is trained for: slight blur + lossy H.264, decoded back to PNG
                sh("ffmpeg", "-hide_banner", "-loglevel", "error", "-i", gt, "-vf",
                   f"scale={GT_W // 2}:{GT_H // 2}:flags=area,gblur=sigma=0.6",
                   "-c:v", "libx264", "-crf", "28", "-frames:v", "1", "-y", f"{low}.mkv")
                sh("ffmpeg", "-hide_banner", "-loglevel", "error", "-i", f"{low}.mkv",
                   "-vf", "format=rgb24", "-y", low)
        out.append((str(low), str(gt)))
    return out, str(shaders)


def build(repo, ref, name, binary):
    d = WORK / name
    sh("git", "worktree", "remove", "--force", d, cwd=repo, check=False)
    shutil.rmtree(d, ignore_errors=True)
    sh("git", "worktree", "add", "--detach", d, ref, cwd=repo)
    r = sh("cargo", "build", "--release", "--quiet", cwd=d, check=False)
    exe = d / "target" / "release" / binary
    return (exe if r.returncode == 0 and exe.exists() else None), r.stderr


def run_case(name, version, kind, imgs, runner, ext, compare=None):
    """runner(low, out) -> (ok, ms, stderr).  Each frame runs twice (determinism)."""
    e = {
        "impl": name,
        "version": version,
        "kind": kind,
        "ms": [],
        "psnr": [],
        "ssim": [],
        "fidelity_psnr": [],
        "deterministic": True,
        "runs_ok": 0,
        "runs": 0,
        "error": None,
        "outputs": [],
    }
    for i, (low, gt) in enumerate(imgs):
        hashes = []
        for k in range(2):
            out = str(
                WORK
                / f"{kind}-{version}-{i}-{k}{ext}".replace("/", "_").replace(" ", "_")
            )
            ok, ms, err = runner(low, out)
            e["runs"] += 1
            if not ok or not Path(out).exists():
                e["error"] = (err.strip().splitlines() or ["failed"])[-1][:200]
                continue
            e["runs_ok"] += 1
            e["ms"].append(ms)
            hashes.append(hashlib.sha256(Path(out).read_bytes()).hexdigest())
            if k == 0:
                e["outputs"].append(out)
                png = out + ".png"
                raw_to_png(out, png) if ext == ".raw" else shutil.copy(out, png)
                p, s = metrics(png, gt)
                e["psnr"].append(p)
                e["ssim"].append(s)
                if compare and i < len(compare[1]):  # (fn, reference outputs)
                    f = compare[0](out, compare[1][i])
                    if f is not None:
                        e["fidelity_psnr"].append(f)
        if len(hashes) == 2 and hashes[0] != hashes[1]:
            e["deterministic"] = False
    for k in ("ms", "psnr", "ssim", "fidelity_psnr"):
        e[k] = round(statistics.median(e[k]), 4) if e[k] else None
    print(
        f"  {name} {version}: {e['ms']} ms/img, PSNR {e['psnr']}, SSIM {e['ssim']}, "
        f"fidelity {e['fidelity_psnr']}, ok {e['runs_ok']}/{e['runs']}",
        file=sys.stderr,
    )
    return e


def cpu_name():
    try:
        for l in open("/proc/cpuinfo"):
            if l.startswith("model name"):
                return l.split(":", 1)[1].strip()
    except OSError:
        pass
    return platform.processor() or "unknown"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", required=True)
    ap.add_argument(
        "--ours", default="HEAD", help="git refs of this repo, space separated"
    )
    ap.add_argument(
        "--theirs", default="v0.2", help="git refs of andraantariksa/Anime4K-rs"
    )
    ap.add_argument("--frames", type=int, default=3)
    ap.add_argument(
        "--source",
        choices=("clean", "degraded"),
        default="clean",
        help="clean = bicubic 2x downscale; degraded = blur + H.264 (real-world anime)",
    )
    ap.add_argument(
        "--preset", help="default: C on clean sources, A on degraded ones (Anime4K guide)"
    )
    a = ap.parse_args()
    a.preset = a.preset or ("C" if a.source == "clean" else "A")
    WORK.mkdir(parents=True, exist_ok=True)
    imgs, shaders = frames(a.frames, a.source)
    env = dict(os.environ, ANIME4K_SHADERS=shaders)
    res = {
        "date": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "commit": sh("git", "rev-parse", "--short", "HEAD", cwd=ROOT).stdout.strip(),
        "machine": {
            "cpu": cpu_name(),
            "threads": os.cpu_count(),
            "os": platform.platform(),
        },
        "frames": len(imgs),
        "input": f"{GT_W // 2}x{GT_H // 2}",
        "output": f"{GT_W}x{GT_H}",
        "preset": a.preset,
        "source": a.source,
        "entries": [],
    }
    add = res["entries"].append

    add(
        run_case(
            "bicubic (ffmpeg)",
            "-",
            "baseline",
            imgs,
            ext=".png",
            runner=lambda low, out: timed(
                "ffmpeg",
                "-hide_banner",
                "-loglevel",
                "error",
                "-i",
                low,
                "-vf",
                f"scale={GT_W}:{GT_H}:flags=bicubic",
                "-y",
                out,
            ),
        )
    )

    for ref in a.ours.split():
        v = sh("git", "describe", "--tags", "--always", ref, cwd=ROOT).stdout.strip()
        exe, err = build(ROOT, ref, f"ours-{v}", "anime4k-rs")
        if not exe:
            add(
                {
                    "impl": "anime4k-rs (native)",
                    "version": v,
                    "kind": "ours",
                    "error": "build failed: " + err.strip()[-200:],
                }
            )
            continue
        run = lambda engine: (
            lambda low, out: timed(
                exe,
                low,
                "--preset",
                a.preset,
                "--hauteur",
                GT_H,
                "--brut",
                "--images",
                1,
                "--moteur",
                engine,
                "--sortie",
                out,
                env=env,
            )
        )

        def compare(x, y):
            m = re.search(
                r"PSNR ([\d.]+|inf)", sh(exe, "comparer", x, y, check=False).stdout
            )
            return float(m.group(1)) if m else None

        # Reference = the original GLSL shaders through libplacebo (Vulkan; llvmpipe on CPU-only machines).
        ref_e = run_case(
            "Anime4K GLSL via libplacebo",
            v,
            "reference",
            imgs,
            run("libplacebo"),
            ".raw",
        )
        if ref_e["runs_ok"]:
            add(ref_e)
        add(
            run_case(
                "anime4k-rs (native)",
                v,
                "ours",
                imgs,
                run("natif"),
                ".raw",
                (compare, ref_e["outputs"])
                if ref_e["runs_ok"] == ref_e["runs"]
                else None,
            )
        )

    theirs = CACHE / "Anime4K-rs"
    if not theirs.exists():
        sh("git", "clone", "--quiet", THEIRS_REPO, theirs)
    sh("git", "fetch", "--quiet", "--tags", cwd=theirs, check=False)
    for ref in a.theirs.split():
        exe, err = build(theirs, ref, f"theirs-{ref}", "anime4k")
        if not exe:
            add(
                {
                    "impl": "andraantariksa/Anime4K-rs",
                    "version": ref,
                    "kind": "theirs",
                    "error": "build failed: " + err.strip()[-200:],
                }
            )
            continue
        e = run_case(
            "andraantariksa/Anime4K-rs",
            ref,
            "theirs",
            imgs,
            lambda low, out, exe=exe: timed(exe, low, out),
            ".png",
        )
        add(e)
    for e in res["entries"]:
        e.pop("outputs", None)
    Path(a.out).write_text(json.dumps(res, indent=2))
    print(a.out)


if __name__ == "__main__":
    main()
