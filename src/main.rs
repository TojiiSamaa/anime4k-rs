// anime4k-rs : conversion « à froid » avec les shaders Anime4K (bloc97, MIT).
// Moteur natif CPU par défaut ; `--moteur libplacebo` = référence (ffmpeg + libplacebo).
mod moteur;

use std::{env, io::{Read, Write}, path::Path, process::{Command, Stdio}, time::Instant};

/// Dossier des .glsl de la release officielle : `ANIME4K_SHADERS`, sinon `./shaders`.
fn shaders() -> String { env::var("ANIME4K_SHADERS").unwrap_or_else(|_| "shaders".into()) }

/// Chaînes HQ officielles d'Anime4K v4 (modes A/B/C et doubles).
fn preset(nom: &str) -> Option<&'static [&'static str]> {
    Some(match nom {
        "A" => &["Clamp_Highlights", "Restore_CNN_VL", "Upscale_CNN_x2_VL", "AutoDownscalePre_x2", "AutoDownscalePre_x4", "Upscale_CNN_x2_M"],
        "B" => &["Clamp_Highlights", "Restore_CNN_Soft_VL", "Upscale_CNN_x2_VL", "AutoDownscalePre_x2", "AutoDownscalePre_x4", "Upscale_CNN_x2_M"],
        "C" => &["Clamp_Highlights", "Upscale_Denoise_CNN_x2_VL", "AutoDownscalePre_x2", "AutoDownscalePre_x4", "Upscale_CNN_x2_M"],
        "A+A" => &["Clamp_Highlights", "Restore_CNN_VL", "Upscale_CNN_x2_VL", "Restore_CNN_M", "AutoDownscalePre_x2", "AutoDownscalePre_x4", "Upscale_CNN_x2_M"],
        "B+B" => &["Clamp_Highlights", "Restore_CNN_Soft_VL", "Upscale_CNN_x2_VL", "AutoDownscalePre_x2", "AutoDownscalePre_x4", "Restore_CNN_Soft_M", "Upscale_CNN_x2_M"],
        "C+A" => &["Clamp_Highlights", "Upscale_Denoise_CNN_x2_VL", "AutoDownscalePre_x2", "AutoDownscalePre_x4", "Restore_CNN_M", "Upscale_CNN_x2_M"],
        _ => return None,
    })
}

fn quitter(msg: &str) -> ! { eprintln!("{msg}"); std::process::exit(1) }
fn usage() -> ! {
    quitter("usage :\n  anime4k-rs <entrée> [--preset A|B|C|A+A|B+B|C+A] [--hauteur 2160] [--sortie f.mkv] [--moteur natif|libplacebo] [--taille S|M|L|VL|UL] [--gpu N|nom] [--encodeur libx265|hevc_nvenc|av1_nvenc|...] [--qualite 20] [--debit 8M] [--images N] [--brut] [--shaders <dossier>]\n  anime4k-rs --lister-gpu\n  anime4k-rs comparer <a.raw> <b.raw>   (gbrpf32le, même taille)")
}

/// (largeur, hauteur, images/s) du premier flux vidéo.
fn sonder(f: &str) -> (usize, usize, String) {
    let o = Command::new("ffprobe").args(["-v", "error", "-select_streams", "v:0", "-show_entries", "stream=width,height,r_frame_rate", "-of", "csv=p=0", f])
        .output().unwrap_or_else(|e| quitter(&format!("ffprobe : {e}")));
    let s = String::from_utf8_lossy(&o.stdout);
    let v: Vec<&str> = s.trim().split(',').collect();
    if v.len() < 3 { quitter(&format!("{f} : pas de flux vidéo")) }
    (v[0].parse().unwrap(), v[1].parse().unwrap(), v[2].to_string())
}

fn comparer(a: &str, b: &str) {
    let (a, b) = (std::fs::read(a).unwrap(), std::fs::read(b).unwrap());
    if a.len() != b.len() { quitter(&format!("tailles différentes : {} / {}", a.len(), b.len())) }
    let (mut se, mut max) = (0f64, 0f32);
    let n = a.len() / 4;
    for (x, y) in a.chunks_exact(4).zip(b.chunks_exact(4)) {
        let d = (f32::from_le_bytes(x.try_into().unwrap()).clamp(0.0, 1.0) - f32::from_le_bytes(y.try_into().unwrap()).clamp(0.0, 1.0)).abs();
        se += (d as f64).powi(2);
        max = max.max(d);
    }
    let psnr = 10.0 * (1.0 / (se / n as f64)).log10();
    println!("PSNR {psnr:.2} dB · écart max {max:.5} ({:.2}/255)", max * 255.0);
}

/// Cartes Vulkan vues par ffmpeg (« N: nom (type) »), dans l'ordre accepté par `--gpu N`.
fn cartes_gpu() -> Vec<String> {
    let o = Command::new("ffmpeg").args(["-hide_banner", "-v", "verbose", "-init_hw_device", "vulkan=vk", "-f", "lavfi", "-i", "nullsrc=s=16x16", "-frames:v", "1", "-f", "null", "-"])
        .output().unwrap_or_else(|e| quitter(&format!("ffmpeg : {e}")));
    let log = String::from_utf8_lossy(&o.stderr);
    log.lines().skip_while(|l| !l.contains("GPU listing:")).skip(1)
        .map(|l| l.rsplit_once("] ").map_or(l, |(_, t)| t).trim().to_string())
        .take_while(|t| t.split_once(": ").is_some_and(|(n, _)| n.parse::<u32>().is_ok())).collect()
}

fn lister_gpu() {
    let cartes = cartes_gpu();
    if cartes.is_empty() { quitter("aucune carte Vulkan trouvée (pilote Vulkan absent ?)") }
    for c in cartes { println!("{c}") }
}

/// `--gpu` (numéro ou partie du nom) -> « vendeur:appareil! » pour MESA_VK_DEVICE_SELECT, à partir d'une
/// ligne « N: nom (type) (0xID) ». Sert quand libplacebo crée lui-même son périphérique (forme 3, issue #5).
// shortcut: vendeur déduit du nom (NVIDIA/AMD/Intel), deux cartes identiques non distinguées ; passer par le bus PCI si besoin.
fn selection_mesa(cartes: &[String], gpu: &str) -> Option<String> {
    // Un numéro ne désigne que l'index (« 2 » est aussi dans « RTX 2080 »), comme vulkan=vk:N dans ffmpeg.
    let numero = gpu.parse::<u32>().is_ok();
    let c = cartes.iter().find(|c| c.split_once(": ").is_some_and(|(n, nom)|
        if numero { n == gpu } else { nom.to_lowercase().contains(&gpu.to_lowercase()) }))?;
    let id = c.rsplit_once("(0x")?.1.trim_end_matches(')');
    let nom = c.to_lowercase();
    let vendeur = if nom.contains("nvidia") { "10de" } else if nom.contains("amd") || nom.contains("radeon") { "1002" } else if nom.contains("intel") { "8086" } else { return None };
    Some(format!("{vendeur}:{id}!"))
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("comparer") {
        return comparer(args.get(1).unwrap_or_else(|| usage()), args.get(2).unwrap_or_else(|| usage()));
    }
    if args.iter().any(|a| a == "--lister-gpu") { return lister_gpu() }
    let entree = args.first().filter(|a| !a.starts_with("--")).unwrap_or_else(|| usage()).clone();
    let opt = |cle: &str| args.windows(2).find(|w| w[0] == cle).map(|w| w[1].clone());
    let brut = args.iter().any(|a| a == "--brut");
    let nom = opt("--preset").unwrap_or_else(|| "A".into());
    let dossier = opt("--shaders").unwrap_or_else(shaders);
    // --gpu (numéro ou partie du nom, cf. --lister-gpu) implique le moteur GPU.
    let gpu = opt("--gpu");
    let moteur = opt("--moteur").unwrap_or_else(|| if gpu.is_some() { "libplacebo" } else { "natif" }.into());
    if gpu.is_some() && moteur != "libplacebo" { quitter("--gpu ne s'applique qu'au moteur libplacebo") }
    let images = opt("--images");
    let enc_args = encodage(&opt("--encodeur").unwrap_or_else(|| "libx265".into()), opt("--qualite").as_deref(), opt("--debit").as_deref());
    let chaine = preset(&nom).unwrap_or_else(|| quitter(&format!("preset inconnu : {nom}")));
    // --taille : la taille des réseaux d'Anime4K à la place des VL (S, M, L plus rapides ; UL ≈ +0,4 dB, ~2× plus lent).
    let taille = opt("--taille").unwrap_or_else(|| "VL".into());
    if !["S", "M", "L", "VL", "UL"].contains(&taille.as_str()) { quitter("--taille : S, M, L, VL ou UL") }
    let sources: Vec<String> = chaine.iter().map(|s| {
        let f = format!("{dossier}/Anime4K_{}.glsl", s.replace("_VL", &format!("_{taille}")));
        std::fs::read_to_string(&f).unwrap_or_else(|e| quitter(&format!("{f} : {e}")))
    }).collect();

    let png = |f: &str| f.to_lowercase().ends_with(".png");
    let sortie = opt("--sortie").unwrap_or_else(|| {
        let p = Path::new(&entree);
        format!("{}/{}.anime4k-{}.{}", p.parent().map_or(".".into(), |d| d.display().to_string()),
            p.file_stem().unwrap().to_string_lossy(), nom.replace('+', "_"),
            if brut { "raw" } else if png(&entree) && moteur == "natif" { "png" } else { "mkv" })
    });
    let oh: usize = opt("--hauteur").and_then(|v| v.parse().ok()).unwrap_or(2160);
    let largeur = |w: usize, h: usize| ((w * oh) as f64 / h as f64 / 2.0).round() as usize * 2;
    let t0 = Instant::now();

    // Image PNG -> PNG/brut : tout en mémoire, sans lancer ffprobe ni ffmpeg (~270 ms de lancement par image).
    if moteur == "natif" && png(&entree) && (brut || png(&sortie)) {
        let img = lire_png(&entree);
        let ow = largeur(img.w, img.h);
        let r = moteur::Moteur::charger(&sources).traiter(img, (ow, oh));
        if brut { std::fs::write(&sortie, vers_gbrp(&r)).unwrap_or_else(|e| quitter(&format!("{sortie} : {e}"))) } else {
            // shortcut: bilinéaire si la chaîne n'atteint pas la taille demandée (le chemin vidéo finit en spline36).
            ecrire_png(&sortie, &if (r.w, r.h) == (ow, oh) { r } else { moteur::reechantillonner(&r, ow, oh) });
        }
        return println!("{sortie} ({:.0} ms)", t0.elapsed().as_secs_f64() * 1e3);
    }

    let (w, h, fps) = sonder(&entree);
    let ow = largeur(w, h);
    let limite: Vec<String> = images.iter().flat_map(|n| ["-frames:v".to_string(), n.clone()]).collect();

    if moteur == "libplacebo" {
        let glsl = env::temp_dir().join(format!("anime4k-{}.glsl", std::process::id()));
        std::fs::write(&glsl, sources.join("\n")).unwrap();
        // Brut : RGBA 16 bits puis conversion en float (hwdownload refuse le float).
        let (fmt, fin) = if brut { ("rgba64le", ",format=gbrpf32le") } else { ("yuv420p10le", "") };
        let lp = format!("libplacebo=w={ow}:h={oh}:custom_shader_path='{}':format={fmt}",
            // Windows : « C:\… » casse le graphe de filtres (« : » et « \ ») ; ffmpeg accepte les « / » et un « : » échappé entre quotes.
            glsl.display().to_string().replace('\\', "/").replace(':', "\\:"));
        let dev = format!("vulkan=vk{}", gpu.as_ref().map_or(String::new(), |g| format!(":{g}")));
        // Formes du graphe, de la plus rapide à la plus tolérante ; la suivante seulement si ffmpeg refuse la précédente.
        // 1. périphérique de ffmpeg + hwupload (~25 % plus rapide, RTX 5080) ;
        // 2. sans hwupload : libplacebo envoie lui-même les images ;
        // 3. sans -init_hw_device : libplacebo crée son propre périphérique Vulkan (seule forme qui marche
        //    sur certaines machines NVIDIA multi-GPU, issue #5) ; --gpu passe alors par MESA_VK_DEVICE_SELECT
        //    (couche Mesa device_select, qui filtre aussi les pilotes NVIDIA), sauf si déjà fixée par l'utilisateur.
        let formes = [(true, format!("hwupload,{lp},hwdownload,format={fmt}{fin}")), (true, format!("{lp}{fin}")), (false, format!("{lp}{fin}"))];
        let mut ok = false;
        for (i, (avec_dev, vf)) in formes.iter().enumerate() {
            let mut c = Command::new("ffmpeg");
            let mesa = match (&gpu, *avec_dev, env::var_os("MESA_VK_DEVICE_SELECT")) {
                (Some(g), false, None) => selection_mesa(&cartes_gpu(), g),
                _ => None,
            };
            if i > 0 { eprintln!("anime4k-rs : nouvel essai, forme {} du graphe libplacebo{}", i + 1,
                match (&mesa, !avec_dev && gpu.is_some()) {
                    (Some(m), _) => format!(" (sans -init_hw_device, carte choisie par MESA_VK_DEVICE_SELECT={m})"),
                    (None, true) => " (sans -init_hw_device : carte de --gpu introuvable ou MESA_VK_DEVICE_SELECT déjà fixée, libplacebo choisit)".into(),
                    _ => String::new(),
                }) }
            if let Some(m) = &mesa { c.env("MESA_VK_DEVICE_SELECT", m); }
            c.args(["-hide_banner", "-loglevel", "error"]);
            if *avec_dev { c.args(["-init_hw_device", &dev, "-filter_hw_device", "vk"]); }
            c.args(["-i", &entree, "-vf", vf]).args(&limite);
            if brut { c.args(["-f", "rawvideo", "-y", &sortie]); } else {
                c.args(["-map", "0:v:0", "-map", "0:a?", "-map", "0:s?", "-map", "0:t?", "-map_chapters", "0"]).args(&enc_args).args(["-y", &sortie]);
            }
            let Ok(o) = c.stderr(Stdio::piped()).output() else { break };
            let err = String::from_utf8_lossy(&o.stderr);
            eprint!("{err}");
            ok = o.status.success();
            // Relance seulement si le graphe n'a pas démarré (pas pour un échec en cours d'encodage).
            let init = ["filter", "libplacebo", "vulkan", "hwupload", "hwdownload"].iter().any(|m| err.to_lowercase().contains(m));
            if ok || !init { break }
        }
        let _ = std::fs::remove_file(&glsl);
        if !ok { quitter("échec ffmpeg/libplacebo") }
        return println!("{sortie} ({:.1} s)", t0.elapsed().as_secs_f64());
    }

    let m = moteur::Moteur::charger(&sources);
    // shortcut: matrice couleur lue dans le flux (repli BT.601 de swscale si non balisé), à forcer si un fichier HD non balisé sort décoloré.
    let mut dec = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-i", &entree, "-map", "0:v:0"]).args(&limite)
        .args(["-vf", "scale=in_color_matrix=auto", "-f", "rawvideo", "-pix_fmt", "gbrpf32le", "-"])
        .stdout(Stdio::piped()).spawn().unwrap_or_else(|e| quitter(&format!("ffmpeg : {e}")));
    let mut lecteur = dec.stdout.take().unwrap();

    let (tx, rx) = std::sync::mpsc::sync_channel::<(usize, usize, Vec<u8>)>(2);
    let enc = {
        let (entree, sortie, fps, enc_args) = (entree.clone(), sortie.clone(), fps.clone(), enc_args.clone());
        std::thread::spawn(move || {
            let mut premier = rx.recv().ok();
            let Some((fw, fh, _)) = premier.as_ref().map(|p| (p.0, p.1, ())) else { return };
            if brut {
                let mut f = std::fs::File::create(&sortie).unwrap();
                while let Some((_, _, d)) = premier.take().or_else(|| rx.recv().ok()) { f.write_all(&d).unwrap() }
                return;
            }
            // Si la chaîne n'atteint pas exactement la taille demandée, ffmpeg finit en spline36 (comme libplacebo).
            let vf = format!("scale={ow}:{oh}:flags=spline:out_color_matrix=bt709:out_range=tv,format=yuv420p10le");
            let mut e = Command::new("ffmpeg")
                .args(["-hide_banner", "-loglevel", "error", "-f", "rawvideo", "-pix_fmt", "gbrpf32le", "-s", &format!("{fw}x{fh}"), "-framerate", &fps, "-i", "-",
                    "-i", &entree, "-map", "0:v", "-map", "1:a?", "-map", "1:s?", "-map", "1:t?", "-map_metadata", "1", "-map_chapters", "1", "-vf", &vf])
                .args(&enc_args).args(["-colorspace", "bt709", "-color_primaries", "bt709", "-color_trc", "bt709", "-y", &sortie])
                .stdin(Stdio::piped()).spawn().unwrap();
            let mut stdin = e.stdin.take().unwrap();
            while let Some((_, _, d)) = premier.take().or_else(|| rx.recv().ok()) {
                if stdin.write_all(&d).is_err() { break }
            }
            drop(stdin);
            if !e.wait().map(|s| s.success()).unwrap_or(false) { quitter("échec de l'encodage") }
        })
    };

    let plan = w * h;
    let mut brute = vec![0u8; plan * 3 * 4];
    let mut n = 0usize;
    while lecteur.read_exact(&mut brute).is_ok() {
        let mut img = moteur::Tex::new(w, h);
        // gbrp : plans G, B, R -> canaux R, G, B, A=1
        moteur::par_lignes(&mut img, |y, ligne| {
            for (p, c) in [(0, 1), (1, 2), (2, 0)] {
                let src = &brute[(p * plan + y * w) * 4..][..w * 4];
                for (d, s) in ligne[c * w..][..w].iter_mut().zip(src.chunks_exact(4)) { *d = f32::from_le_bytes(s.try_into().unwrap()) }
            }
            ligne[3 * w..].fill(1.0);
        });
        let r = m.traiter(img, (ow, oh));
        if tx.send((r.w, r.h, vers_gbrp(&r))).is_err() { break }
        n += 1;
        eprint!("\r{n} images · {:.2} img/s", n as f64 / t0.elapsed().as_secs_f64());
    }
    drop(tx);
    let _ = dec.wait();
    enc.join().unwrap();
    eprintln!();
    println!("{sortie} ({n} images en {:.1} s)", t0.elapsed().as_secs_f64());
}

/// Texture (lignes R, G, B, A) -> gbrpf32le (plans G, B, R), le format brut échangé avec ffmpeg.
fn vers_gbrp(r: &moteur::Tex) -> Vec<u8> {
    let mut out = vec![0u8; r.w * r.h * 12];
    let lignes_par = r.h.div_ceil(std::thread::available_parallelism().map_or(1, |n| n.get()));
    std::thread::scope(|s| {
        for (plan_sortie, c) in out.chunks_mut(r.w * r.h * 4).zip([1, 2, 0]) {
            for (k, bloc) in plan_sortie.chunks_mut(lignes_par * r.w * 4).enumerate() {
                s.spawn(move || for (j, ligne) in bloc.chunks_mut(r.w * 4).enumerate() {
                    let src = &r.d[((k * lignes_par + j) * 4 + c) * r.w..][..r.w];
                    for (d, v) in ligne.chunks_exact_mut(4).zip(src) { d.copy_from_slice(&v.to_le_bytes()) }
                });
            }
        }
    });
    out
}

/// PNG 8 bits (gris, RVB, avec ou sans alpha, palette) -> texture ; valeurs / 255 comme swscale (rgb24 -> gbrpf32le).
fn lire_png(f: &str) -> moteur::Tex {
    let err = |e: &dyn std::fmt::Display| -> ! { quitter(&format!("{f} : {e}")) };
    let fichier = std::fs::File::open(f).unwrap_or_else(|e| err(&e));
    let mut d = png::Decoder::new(std::io::BufReader::new(fichier));
    d.set_transformations(png::Transformations::normalize_to_color8());
    let mut lu = d.read_info().unwrap_or_else(|e| err(&e));
    let mut buf = vec![0; lu.output_buffer_size().unwrap_or_else(|| err(&"image trop grande"))];
    let info = lu.next_frame(&mut buf).unwrap_or_else(|e| err(&e));
    let (w, h, n) = (info.width as usize, info.height as usize, info.color_type.samples());
    let mut t = moteur::Tex::new(w, h);
    moteur::par_lignes(&mut t, |y, ligne| {
        let src = &buf[y * info.line_size..][..w * n];
        for x in 0..w {
            let p = &src[x * n..];
            for c in 0..3 { ligne[c * w + x] = p[if n < 3 { 0 } else { c }] as f32 / 255.0 }
        }
        ligne[3 * w..].fill(1.0);
    });
    t
}

/// Texture -> PNG RVB 8 bits (arrondi, écrêté à [0, 1]).
fn ecrire_png(f: &str, r: &moteur::Tex) {
    let mut rgb = vec![0u8; r.w * r.h * 3];
    for (y, l) in rgb.chunks_exact_mut(r.w * 3).enumerate() {
        let src = &r.d[y * r.w * 4..];
        for x in 0..r.w { for c in 0..3 { l[x * 3 + c] = (src[c * r.w + x].clamp(0.0, 1.0) * 255.0).round() as u8 } }
    }
    let fichier = std::fs::File::create(f).unwrap_or_else(|e| quitter(&format!("{f} : {e}")));
    let mut e = png::Encoder::new(std::io::BufWriter::new(fichier), r.w as u32, r.h as u32);
    e.set_color(png::ColorType::Rgb);
    e.set_depth(png::BitDepth::Eight);
    e.set_compression(png::Compression::Fast);
    e.write_header().and_then(|mut w| w.write_image_data(&rgb)).unwrap_or_else(|e| quitter(&format!("{f} : {e}")));
}

/// Arguments ffmpeg de l'encodeur vidéo : `--encodeur` (libx265 par défaut), `--qualite` (CRF/CQ, 20) ou `--debit` (ex. 8M).
fn encodage(enc: &str, qualite: Option<&str>, debit: Option<&str>) -> Vec<String> {
    let q = qualite.unwrap_or("20");
    let nvenc = enc.ends_with("_nvenc");
    // H.264 n'a pas de 10 bits matériel ; NVENC veut du p010 en 10 bits.
    let pix = if enc.contains("264") { "yuv420p" } else if nvenc { "p010le" } else { "yuv420p10le" };
    let mut a: Vec<&str> = vec!["-c:v", enc, "-pix_fmt", pix];
    // shortcut: NVENC encode sur la 1re carte NVIDIA ; à relier à --gpu si quelqu'un a plusieurs cartes NVIDIA.
    match (nvenc, debit) {
        (true, Some(d)) => a.extend(["-preset", "p5", "-rc", "vbr", "-b:v", d]),
        (true, None) => a.extend(["-preset", "p5", "-rc", "vbr", "-cq", q, "-b:v", "0"]),
        (false, Some(d)) => a.extend(["-b:v", d]),
        (false, None) if enc.starts_with("libx26") || enc == "libsvtav1" => a.extend(["-crf", q]),
        _ => {}
    }
    if enc.starts_with("libx26") { a.extend(["-preset", "medium"]) }
    a.extend(["-c:a", "copy", "-c:s", "copy"]);
    a.into_iter().map(String::from).collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn selection_carte_mesa() {
        // Liste réelle de la machine de l'issue #5.
        let c: Vec<String> = ["0: NVIDIA GeForce RTX 2080 Ti (discrete) (0x1e07)", "1: NVIDIA GeForce RTX 2070 SUPER (discrete) (0x1e84)",
            "2: Intel(R) Graphics (RPL-S) (integrated) (0xa780)"].map(String::from).into();
        let s = |g: &str| super::selection_mesa(&c, g);
        assert_eq!(s("1").as_deref(), Some("10de:1e84!"));
        assert_eq!(s("2080 ti").as_deref(), Some("10de:1e07!"));
        assert_eq!(s("2070"), None); // nombre = index, comme vulkan=vk:N
        assert_eq!(s("2").as_deref(), Some("8086:a780!"));
        assert_eq!(s("7"), None);
    }

    #[test]
    fn options_encodeur() {
        let j = |v: Vec<String>| v.join(" ");
        assert!(j(super::encodage("libx265", None, None)).contains("-pix_fmt yuv420p10le -crf 20"));
        assert!(j(super::encodage("hevc_nvenc", Some("24"), None)).contains("-pix_fmt p010le -preset p5 -rc vbr -cq 24 -b:v 0"));
        assert!(j(super::encodage("h264_nvenc", None, Some("8M"))).contains("-pix_fmt yuv420p -preset p5 -rc vbr -b:v 8M"));
        assert!(!j(super::encodage("libx264", None, Some("8M"))).contains("-crf"));
    }

    #[test]
    fn presets_connus_et_analysables() {
        for p in ["A", "B", "C", "A+A", "B+B", "C+A"] {
            let src: Vec<String> = super::preset(p).unwrap().iter()
                .map(|s| std::fs::read_to_string(format!("{}/Anime4K_{s}.glsl", super::shaders())).expect("shader absent (ANIME4K_SHADERS ?)")).collect();
            super::moteur::Moteur::charger(&src); // panique sur toute ligne non reconnue
            for t in ["S", "M", "L", "UL"] {
                let v: Vec<String> = super::preset(p).unwrap().iter()
                    .map(|s| std::fs::read_to_string(format!("{}/Anime4K_{}.glsl", super::shaders(), s.replace("_VL", &format!("_{t}")))).unwrap()).collect();
                super::moteur::Moteur::charger(&v);
            }
        }
        assert!(super::preset("Z").is_none());
    }
}
