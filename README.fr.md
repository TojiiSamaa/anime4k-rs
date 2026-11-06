<p align="center">
  <a href="https://github.com/TojiiSamaa/anime4k-rs/actions/workflows/perf-bench.yml">
    <img src="bench/bench.svg" alt="Banc : temps par image, qualité et fiabilité, mis à jour par la CI à chaque push" width="900">
    <img src="bench/bench-degraded.svg" alt="Même banc sur une source floutée et compressée en H.264" width="900">
  </a>
</p>
<p align="center">
  <a href="https://tojiisamaa.github.io/anime4k-rs/bench/"><img src="bench/btn-resultats.svg" alt="Résultats du banc (triables)" height="64"></a>
  <a href="https://github.com/TojiiSamaa/anime4k-rs/actions/workflows/perf-bench.yml"><img src="bench/btn-relancer.svg" alt="Relancer le banc" height="64"></a>
</p>

# anime4k-rs

[English](README.md) · **Français**

[Anime4K](https://github.com/bloc97/Anime4K) (de bloc97) est un ensemble de shaders libres d'upscaling et de débruitage d'anime en temps réel, de grande qualité, pour mpv. **anime4k-rs garde ces shaders tels quels et ajoute ce qu'ils ne savent pas faire : convertir un fichier vidéo à l'avance.** Le résultat se lit alors partout, téléphone compris, sans aucun support des shaders.

- **Lecture en direct** (mpv, Plex HTPC) : rien ne change. On garde les shaders officiels, que le lecteur exécute sur la carte graphique. Voir [Lecture en direct](#lecture-en-direct).
- **Conversion à froid** (cet outil) : `anime4k-rs episode.mkv` produit `episode.anime4k-A.mkv`, traité avec le preset A, avec le son, les sous-titres, les polices et les chapitres recopiés.

## Ce qui change par rapport à Anime4K

Les réseaux de neurones **ne sont pas modifiés** : mêmes couches, mêmes poids, même ordre des passes que la release v4.0.1. Les poids sont lus à l'exécution dans les `.glsl` officiels, donc aucun n'est recopié dans ce dépôt.

| Domaine | Anime4K (origine) | anime4k-rs |
|---|---|---|
| Où ça tourne | shaders GLSL dans mpv, sur le GPU, pendant la lecture | un outil en ligne de commande qui écrit une nouvelle vidéo |
| Moteur GPU | mpv / libplacebo | `--moteur libplacebo` : les mêmes shaders via ffmpeg + libplacebo (Vulkan) |
| Moteur CPU | aucun | `--moteur natif` (défaut) : interpréteur des shaders en Rust pur (seule dépendance : le crate `png`) |
| Modes | raccourcis dans `input.conf` | `--preset A`, `B`, `C`, `A+A`, `B+B`, `C+A` (mêmes chaînes que les modes HQ officiels) |
| Taille de sortie | taille de l'écran | `--hauteur N` (2160 par défaut), la largeur suit les proportions |
| Fichier de sortie | aucun | HEVC 10 bits (libx265, CRF 20) par défaut, ou NVENC / AV1 via `--encodeur` ; son, sous-titres, pièces jointes et chapitres recopiés |

### Le moteur natif, fonction par fonction (`src/moteur.rs`)

| Fonction | Rôle |
|---|---|
| `Moteur::charger` | Lit le format des shaders mpv (`//!HOOK`, `BIND`, `SAVE`, `WIDTH`, `HEIGHT`, `WHEN`, `COMPONENTS`) et fait de chaque passe une opération typée. Il s'arrête net sur toute ligne inconnue : un shader qu'il ne sait pas exécuter n'est jamais exécuté de travers en silence. |
| `rpn` | Évalue les expressions en notation polonaise inverse des tailles et des `WHEN` (`NATIVE`, `OUTPUT`, `HOOKED`, textures sauvées). Les passes comme `AutoDownscalePre` s'activent donc exactement comme dans mpv. |
| `Moteur::traiter` | Exécute les passes dans l'ordre des points d'accroche (`MAIN`, puis `PREKERNEL`) et garde chaque texture sauvée. `A4K_PROFIL=1` affiche le temps de chaque passe. |
| `conv` / `noyau_avx2` | Les convolutions 3×3 et 1×1, avec entrées CReLU et résidu final. Les lignes sont réparties sur tous les cœurs et traitées par tuiles adaptées au cache. La boucle interne est en AVX2/FMA (24 pixels par itération), avec un repli scalaire. |
| `d2s` | Depth-to-space (pixel shuffle) des upscalers ×2, avec le résidu bilinéaire de l'entrée. |
| `stats` / `clamp` | `Clamp_Highlights` : maximum de luminance sur 5 points, à l'horizontale puis à la verticale, puis écrêtage de la sortie. |
| `bil_ligne`, `Taps` | Échantillonnage bilinéaire à coefficients précalculés, à la place du filtrage de textures. |

La sortie native rejoint la référence GPU à **PSNR 54,5 dB, écart max 1,4/255** sur une mire sans perte. L'écart restant vient du fp16 du GPU face à notre f32.

### Presets

| Preset | Chaîne (modes HQ officiels d'Anime4K v4) |
|---|---|
| `A` (défaut) | Clamp_Highlights → Restore_CNN_VL → Upscale_CNN_x2_VL → AutoDownscalePre_x2/x4 → Upscale_CNN_x2_M |
| `B` | Clamp_Highlights → Restore_CNN_Soft_VL → Upscale_CNN_x2_VL → AutoDownscalePre_x2/x4 → Upscale_CNN_x2_M |
| `C` | Clamp_Highlights → Upscale_Denoise_CNN_x2_VL → AutoDownscalePre_x2/x4 → Upscale_CNN_x2_M |
| `A+A` | A, plus Restore_CNN_M après le premier upscale |
| `B+B` | B, plus Restore_CNN_Soft_M avant le dernier upscale |
| `C+A` | C, plus Restore_CNN_M avant le dernier upscale |

**Le preset doit suivre la source, c'est ce qui compte le plus.** Mesures sur Pepper&Carrot, de 540p vers 1080p (PSNR face à l'original) :

| Source | bicubique | A | C | C + `--taille UL` |
|---|---|---|---|---|
| propre (réduction sans perte) | 34,42 | 33,72 | 38,61 | **39,08** |
| floutée + H.264 | 29,37 | **30,84** | 29,98 | 29,98 |

A restaure l'image (défloutage, renforcement) avant l'upscale. Sur une source propre il n'y a rien à restaurer et il perd des points ; sur une source compressée c'est le meilleur. La première carte mesure C sur une source propre, la seconde A sur une source dégradée.

D'après le guide d'Anime4K : A convient à la plupart des anime 1080p flous et compressés, B aux sources à peine floues ou qui présentent du ringing, C aux sources déjà nettes (il ne fait qu'upscaler et débruiter).

## Utilisation

```sh
# 1. shaders officiels v4.0.1
curl -LO https://github.com/bloc97/Anime4K/releases/download/v4.0.1/Anime4K_v4.0.zip
unzip Anime4K_v4.0.zip -d shaders

# 2. compilation
cargo build --release

# 3. conversion
anime4k-rs episode.mkv                                 # preset A, 2160p, moteur natif (CPU)
anime4k-rs episode.mkv --preset B+B --hauteur 1440     # fichier plus léger pour un téléphone
anime4k-rs episode.mkv --moteur libplacebo             # sur une machine avec GPU (bien plus rapide)
anime4k-rs image.png --preset C --hauteur 2160         # PNG vers PNG, en mémoire (sans ffmpeg)
```

| Option | Défaut | Rôle |
|---|---|---|
| `--preset` | `A` | `A`, `B`, `C`, `A+A`, `B+B`, `C+A` |
| `--hauteur` | `2160` | hauteur de sortie (la largeur suit) |
| `--taille` | `VL` | `VL` (modes HQ officiels) ; `S`, `M`, `L` : réseaux plus petits, 2 à 3× plus rapides pour −0,4 à −1,5 dB (`S` bat encore l'autre portage Rust sur les deux sources du banc) ; `UL` : les plus gros réseaux d'Anime4K, environ +0,4 dB et ~2× plus lents |
| `--sortie` | `<entrée>.anime4k-<preset>.mkv` | fichier de sortie |
| `--moteur` | `natif` | `natif` (CPU) ou `libplacebo` (GPU via Vulkan) |
| `--gpu` | première carte Vulkan | carte graphique qui fait le rendu : son numéro ou une partie de son nom (`--gpu 1`, `--gpu "RTX 5080"`). Implique `--moteur libplacebo` |
| `--encodeur` | `libx265` | n'importe quel encodeur vidéo de ffmpeg : `libx265`, `libsvtav1`, `libx264`, ou sur NVIDIA `hevc_nvenc`, `av1_nvenc`, `h264_nvenc` (bien plus rapides). 10 bits sauf en H.264 |
| `--qualite` | `20` | qualité constante : CRF pour libx26x/SVT-AV1, CQ pour NVENC (plus bas = meilleur et plus lourd) |
| `--debit` | aucun | débit cible à la place de la qualité constante (`--debit 8M`) |
| `--images N` | toutes | s'arrêter après N images (tests) |
| `--brut` | non | écrire des images `gbrpf32le` brutes au lieu d'une vidéo (bancs) |
| `--shaders` | `$ANIME4K_SHADERS` ou `./shaders` | dossier des `.glsl` |

`anime4k-rs --lister-gpu` liste les cartes graphiques acceptées par `--gpu` :

```
0: NVIDIA GeForce RTX 5080 (discrete) (0x2c02)
1: AMD Radeon(TM) Graphics (integrated) (0x13c0)
```

`anime4k-rs comparer a.raw b.raw` affiche le PSNR et l'écart max entre deux sorties `--brut`.

**Prérequis :** `ffmpeg` et `ffprobe` (compilés avec libplacebo et Vulkan pour `--moteur libplacebo`), et Rust 1.85+ (édition 2024).

## Vitesse

Mesures avec le preset A, d'une source 1080p vers du 2160p :

| Machine | Moteur | Vitesse | Épisode de 24 min |
|---|---|---|---|
| 2× Xeon E5-2670 v3 (48 threads), sans GPU | libplacebo sur llvmpipe | 0,40 img/s | ~24 h |
| idem | **natif** | **0,86 img/s** | ~11 h |
| RTX 5080 | libplacebo | ~88 img/s | — |
| RTX 5080 | libplacebo + HEVC NVENC | ~58 img/s | **~10 min** |

Avec un GPU, prendre `--moteur libplacebo`. Le moteur natif sert aux machines qui n'en ont pas.

## Banc et CI

`.github/workflows/perf-bench.yml` tourne à chaque push, tag et pull request :

1. Il compile et lance les tests.
2. Il exécute `bench/bench.py` deux fois (source propre avec le preset C, puis source floutée et compressée en H.264 avec le preset A) sur des extraits 1920×1080 de [Pepper&Carrot](https://www.peppercarrot.com) (illustration 2D de David Revoy, CC BY 4.0). Chaque image est réduite 2×, puis agrandie à nouveau par chaque implémentation et comparée à l'original. Le script mesure :
   - le **temps par image** ;
   - la **qualité** : PSNR et SSIM face à l'image d'origine ;
   - la **fidélité** : PSNR entre notre moteur natif et les shaders GLSL d'origine exécutés par libplacebo ;
   - la **fiabilité** : chaque image passe deux fois et doit donner une sortie identique au bit près, et le taux de réussite est indiqué.
3. Il compare plusieurs versions d'un coup. Une PR compare `main` à la tête de la PR et poste l'écart en commentaire. Un lancement manuel accepte n'importe quelle liste de nos tags ou commits, et des versions d'[andraantariksa/Anime4K-rs](https://github.com/andraantariksa/Anime4K-rs). Ce projet est le portage Rust cité dans le README d'Anime4K, et il implémente l'ancien algorithme v1 (« push ») plutôt que le CNN.
4. Sur `main`, il ajoute le résultat à `bench/history/` et redessine la carte en haut de cette page.

En local : `python3 bench/bench.py --source clean --out result.json --ours "HEAD v0.1.0" --theirs "v0.1 v0.2"`, puis `python3 bench/render.py result.json --svg bench.svg`.

## Lecture en direct

Pour le temps réel, on utilise les shaders d'origine. Ce chemin ne change pas et ne demande aucune conversion. Copier les `.glsl` dans `~~/shaders/`. Ajouter ensuite à `input.conf` les raccourcis du [guide mpv officiel](https://github.com/bloc97/Anime4K/blob/master/md/GLSL_Instructions_Advanced.md), par exemple pour le mode A :

```
CTRL+1 no-osd change-list glsl-shaders set "~~/shaders/Anime4K_Clamp_Highlights.glsl:~~/shaders/Anime4K_Restore_CNN_VL.glsl:~~/shaders/Anime4K_Upscale_CNN_x2_VL.glsl:~~/shaders/Anime4K_AutoDownscalePre_x2.glsl:~~/shaders/Anime4K_AutoDownscalePre_x4.glsl:~~/shaders/Anime4K_Upscale_CNN_x2_M.glsl"; show-text "Anime4K: Mode A (HQ)"
CTRL+0 no-osd change-list glsl-shaders clr ""; show-text "GLSL shaders cleared"
```

Pour choisir la carte graphique qui exécute les shaders pendant la lecture, l'indiquer dans `mpv.conf`. L'une ou l'autre ligne convient, selon l'API de rendu :

```
gpu-api=vulkan
vulkan-device="NVIDIA GeForce RTX 5080"   # nom exact, liste : mpv --vulkan-device=help
# ou, avec l'API Windows par défaut :
d3d11-adapter="NVIDIA"                    # partie du nom de la carte
```

Plex HTPC repose sur mpv et accepte donc les mêmes `mpv.conf` et `input.conf`. Les autres applis Plex, et les téléphones, lisent les fichiers convertis par anime4k-rs.

## Licence

MIT, voir [`LICENSE`](LICENSE). Anime4K, ses shaders et ses réseaux sont © bloc97, sous licence MIT, voir [`LICENSE-Anime4K`](LICENSE-Anime4K). Les images du banc sont des extraits de Pepper&Carrot de David Revoy, CC BY 4.0, www.peppercarrot.com. Elles sont téléchargées à l'exécution et ne sont pas stockées dans ce dépôt.
