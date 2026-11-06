<p align="center">
  <a href="https://github.com/TojiiSamaa/anime4k-rs/actions/workflows/perf-bench.yml">
    <img src="bench/bench.svg" alt="Benchmark: time per image, quality and reliability, updated by CI on every push" width="900">
    <img src="bench/bench-degraded.svg" alt="Same benchmark on a blurred, H.264-compressed source" width="900">
  </a>
</p>
<p align="center">
  <a href="https://tojiisamaa.github.io/anime4k-rs/bench/"><img src="bench/btn-results.svg" alt="Benchmark results (sortable)" height="64"></a>
  <a href="https://github.com/TojiiSamaa/anime4k-rs/actions/workflows/perf-bench.yml"><img src="bench/btn-rerun.svg" alt="Re-run the bench" height="64"></a>
</p>

# anime4k-rs

**English** · [Français](README.fr.md)

[Anime4K](https://github.com/bloc97/Anime4K) (by bloc97) is a set of open-source, high-quality real-time anime upscaling and denoising shaders for mpv. **anime4k-rs keeps those shaders exactly as they are and adds what they cannot do: convert a video file ahead of time**, so the upscaled result plays anywhere, including on a phone, with no shader support.

- **Live playback** (mpv, Plex HTPC): unchanged. Use the official shaders; the player runs them on your GPU. See [Live playback](#live-playback).
- **Offline conversion** (this tool): `anime4k-rs episode.mkv` produces `episode.anime4k-A.mkv`, upscaled with preset A, with audio, subtitles, fonts and chapters copied over.

## What changed compared with Anime4K

The neural networks are **not modified**: same layers, same weights, same pipeline order as the v4.0.1 release. The weights are read from the official `.glsl` files at run time, so none of them are copied into this repository.

| Area | Anime4K (original) | anime4k-rs |
|---|---|---|
| Where it runs | GLSL shaders inside mpv, on the GPU, while you watch | A CLI that writes a new video file |
| GPU engine | mpv / libplacebo | `--moteur libplacebo`: the same shaders through ffmpeg + libplacebo (Vulkan) |
| CPU engine | none | `--moteur natif` (default): a pure-Rust interpreter of the shader files (only dependency: the `png` crate) |
| Modes | key bindings in `input.conf` | `--preset A`, `B`, `C`, `A+A`, `B+B`, `C+A` (same chains as the official HQ modes) |
| Output size | screen size | `--hauteur N` (default 2160), width follows the aspect ratio |
| Output file | none | HEVC 10-bit (libx265, CRF 20) by default, or NVENC / AV1 via `--encodeur`; audio, subtitles, attachments and chapters copied |

### The native engine, function by function (`src/moteur.rs`)

| Function | Role |
|---|---|
| `Moteur::charger` | Parses the mpv user-shader format (`//!HOOK`, `BIND`, `SAVE`, `WIDTH`, `HEIGHT`, `WHEN`, `COMPONENTS`) and turns each pass into a typed operation. It panics on any line it does not understand, so a shader it cannot run is never silently mis-run. |
| `rpn` | Evaluates the reverse-Polish size and `WHEN` expressions (`NATIVE`, `OUTPUT`, `HOOKED`, saved textures), so passes such as `AutoDownscalePre` switch on and off exactly as they do in mpv. |
| `Moteur::traiter` | Runs the passes in hook order (`MAIN`, then `PREKERNEL`) and keeps every saved texture. `A4K_PROFIL=1` prints the time taken by each pass. |
| `conv` / `noyau_avx2` | The 3×3 and 1×1 convolutions with CReLU inputs and the final residual add. Rows are split across all cores and processed in cache-sized tiles. The inner loop uses AVX2/FMA (24 pixels per iteration), with a scalar fallback. |
| `d2s` | Depth-to-space (pixel shuffle) for the ×2 upscalers, plus the bilinear residual of the input. |
| `stats` / `clamp` | `Clamp_Highlights`: a 5-tap luma maximum horizontally then vertically, then a clamp of the output. |
| `bil_ligne`, `Taps` | Bilinear sampling with precomputed taps, which replaces texture filtering. |

The native output matches the GPU reference at **PSNR 54.5 dB, max error 1.4/255** on a lossless test pattern. The remaining gap is the GPU's fp16 against our f32.

### Presets

| Preset | Chain (official Anime4K v4 HQ modes) |
|---|---|
| `A` (default) | Clamp_Highlights → Restore_CNN_VL → Upscale_CNN_x2_VL → AutoDownscalePre_x2/x4 → Upscale_CNN_x2_M |
| `B` | Clamp_Highlights → Restore_CNN_Soft_VL → Upscale_CNN_x2_VL → AutoDownscalePre_x2/x4 → Upscale_CNN_x2_M |
| `C` | Clamp_Highlights → Upscale_Denoise_CNN_x2_VL → AutoDownscalePre_x2/x4 → Upscale_CNN_x2_M |
| `A+A` | A, plus Restore_CNN_M after the first upscale |
| `B+B` | B, plus Restore_CNN_Soft_M before the last upscale |
| `C+A` | C, plus Restore_CNN_M before the last upscale |

**Pick the preset for the source, it matters more than anything else.** Measured on Pepper&Carrot, 540p → 1080p (PSNR vs the original):

| Source | bicubic | A | C | C + `--taille UL` |
|---|---|---|---|---|
| clean (lossless downscale) | 34.42 | 33.72 | 38.61 | **39.08** |
| blurred + H.264 | 29.37 | **30.84** | 29.98 | 29.98 |

A restores (deblurs, sharpens) before upscaling: on a clean source there is nothing to restore and it loses points; on a compressed source it is the best. The top card benches C on clean sources, the second A on degraded ones.

From the Anime4K guide: A suits most 1080p anime with blur and compression, B suits sources that are only slightly blurry or that ring, and C suits sources that are already sharp (it upscales and denoises only).

## Usage

```sh
# 1. official shaders v4.0.1
curl -LO https://github.com/bloc97/Anime4K/releases/download/v4.0.1/Anime4K_v4.0.zip
unzip Anime4K_v4.0.zip -d shaders

# 2. build
cargo build --release

# 3. convert
anime4k-rs episode.mkv                                 # preset A, 2160p, native CPU engine
anime4k-rs episode.mkv --preset B+B --hauteur 1440     # lighter file for a phone
anime4k-rs episode.mkv --moteur libplacebo             # on a machine with a GPU (much faster)
anime4k-rs image.png --preset C --hauteur 2160         # PNG in, PNG out, in memory (no ffmpeg)
```

| Option | Default | Meaning |
|---|---|---|
| `--preset` | `A` | `A`, `B`, `C`, `A+A`, `B+B`, `C+A` |
| `--hauteur` | `2160` | output height (width follows) |
| `--taille` | `VL` | `VL` (official HQ modes); `S`, `M`, `L`: smaller networks, 2–3× faster for −0.4 to −1.5 dB (`S` still beats the other Rust port on both bench sources); `UL`: Anime4K's largest networks, about +0.4 dB and ~2× slower |
| `--sortie` | `<input>.anime4k-<preset>.mkv` | output file |
| `--moteur` | `natif` | `natif` (CPU) or `libplacebo` (GPU through Vulkan) |
| `--gpu` | first Vulkan GPU | which graphics card renders: its number, or part of its name (`--gpu 1`, `--gpu "RTX 5080"`). Implies `--moteur libplacebo` |
| `--encodeur` | `libx265` | any ffmpeg video encoder: `libx265`, `libsvtav1`, `libx264`, or on NVIDIA `hevc_nvenc`, `av1_nvenc`, `h264_nvenc` (much faster). 10-bit except H.264 |
| `--qualite` | `20` | constant quality: CRF for libx26x/SVT-AV1, CQ for NVENC (lower = better, bigger) |
| `--debit` | none | target bitrate instead of constant quality (`--debit 8M`) |
| `--images N` | all | stop after N frames (for tests) |
| `--brut` | off | write raw `gbrpf32le` frames instead of a video (for benchmarks) |
| `--shaders` | `$ANIME4K_SHADERS` or `./shaders` | folder containing the `.glsl` files |

`anime4k-rs --lister-gpu` lists the graphics cards that `--gpu` accepts:

```
0: NVIDIA GeForce RTX 5080 (discrete) (0x2c02)
1: AMD Radeon(TM) Graphics (integrated) (0x13c0)
```

`anime4k-rs comparer a.raw b.raw` prints the PSNR and the maximum difference between two `--brut` outputs.

**Requirements:** `ffmpeg` and `ffprobe` (built with libplacebo and Vulkan for `--moteur libplacebo`), and Rust 1.85+ (edition 2024).

## Speed

Measured with preset A on a 1080p source scaled to 2160p:

| Machine | Engine | Speed | 24-min episode |
|---|---|---|---|
| 2× Xeon E5-2670 v3 (48 threads), no GPU | libplacebo on llvmpipe | 0.40 img/s | ~24 h |
| same | **native** | **0.86 img/s** | ~11 h |
| RTX 5080 | libplacebo | ~88 img/s | — |
| RTX 5080 | libplacebo + HEVC NVENC | ~58 img/s | **~10 min** |

With a GPU, use `--moteur libplacebo`. The native engine is for machines without one.

## Benchmark and CI

`.github/workflows/perf-bench.yml` runs on every push, tag and pull request:

1. It builds and tests the code.
2. It runs `bench/bench.py` twice (a clean source with preset C, then a blurred + H.264 source with preset A) on 1920×1080 crops of [Pepper&Carrot](https://www.peppercarrot.com) (2D illustration by David Revoy, CC BY 4.0). Each image is downscaled 2×, then upscaled back by every implementation and compared with the original. The script measures:
   - **time per image**;
   - **quality**: PSNR and SSIM against the original frame;
   - **fidelity**: PSNR between our native engine and the original GLSL shaders run by libplacebo;
   - **reliability**: each image runs twice and must give a bit-identical output, and the success rate is reported.
3. It compares several versions at once. A pull request benches `main` against the PR head and posts the difference as a comment. A manual run accepts any list of our tags or commits and of [andraantariksa/Anime4K-rs](https://github.com/andraantariksa/Anime4K-rs) refs. That project is the Rust port listed in the Anime4K README, and it implements the older v1 push algorithm rather than the CNN.
4. On `main`, it adds the result to `bench/history/` and redraws the card at the top of this page.

Run it locally with `python3 bench/bench.py --source clean --out result.json --ours "HEAD v0.1.0" --theirs "v0.1 v0.2"`, then `python3 bench/render.py result.json --svg bench.svg`.

## Live playback

For real-time playback, use the original shaders. That path is unchanged and needs no conversion. Copy the `.glsl` files to `~~/shaders/`. Then add the bindings from the [official mpv guide](https://github.com/bloc97/Anime4K/blob/master/md/GLSL_Instructions_Advanced.md) to `input.conf`, for example mode A:

```
CTRL+1 no-osd change-list glsl-shaders set "~~/shaders/Anime4K_Clamp_Highlights.glsl:~~/shaders/Anime4K_Restore_CNN_VL.glsl:~~/shaders/Anime4K_Upscale_CNN_x2_VL.glsl:~~/shaders/Anime4K_AutoDownscalePre_x2.glsl:~~/shaders/Anime4K_AutoDownscalePre_x4.glsl:~~/shaders/Anime4K_Upscale_CNN_x2_M.glsl"; show-text "Anime4K: Mode A (HQ)"
CTRL+0 no-osd change-list glsl-shaders clr ""; show-text "GLSL shaders cleared"
```

To choose which graphics card runs the shaders during playback, set it in `mpv.conf`. Either line works, depending on the rendering API:

```
gpu-api=vulkan
vulkan-device="NVIDIA GeForce RTX 5080"   # exact name, as listed by: mpv --vulkan-device=help
# or, with the default Windows API:
d3d11-adapter="NVIDIA"                    # part of the adapter name
```

Plex HTPC is built on mpv, so it accepts the same `mpv.conf` and `input.conf`. Other Plex apps, and phones, play the files converted by anime4k-rs.

## License

MIT, see [`LICENSE`](LICENSE). Anime4K, its shaders and its networks are © bloc97, MIT, see [`LICENSE-Anime4K`](LICENSE-Anime4K). The benchmark images are crops of Pepper&Carrot by David Revoy, CC BY 4.0, www.peppercarrot.com. They are downloaded at run time and are not stored in this repository.
