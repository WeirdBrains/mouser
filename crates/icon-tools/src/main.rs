use anyhow::{bail, Context, Result};
use ico::{IconDir, IconImage};
use icns::{IconFamily, Image as IcnsImage};
use resvg::render;
use std::fs::{create_dir_all, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use tiny_skia::Pixmap;
use usvg::{Options, Tree};

const SIZES: &[u32] = &[16, 24, 32, 48, 64, 128, 256, 512, 1024];
const SMALL_SIZES: &[u32] = &[16, 32];

fn main() -> Result<()> {
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).ancestors().nth(2).unwrap().to_path_buf();
    let master_svg = repo_root.join("assets/icon/mouser.svg");
    let small_svg = repo_root.join("assets/icon/mouser-small.svg");

    let out_png = repo_root.join("resources/icons/png");
    let out_hicolor = repo_root.join("resources/icons/hicolor");
    let out_win = repo_root.join("resources/windows");
    let out_mac = repo_root.join("resources/macos");
    create_dir_all(&out_png)?;
    create_dir_all(&out_hicolor)?;
    create_dir_all(&out_win)?;
    create_dir_all(&out_mac)?;

    // Prepare SVG trees
    let opt = Options::default();
    let master = load_svg(&master_svg, &opt)?;
    let small = load_svg(&small_svg, &opt)?;

    // Render PNGs
    for &size in SIZES {
        let use_small = SMALL_SIZES.contains(&size);
        let tree = if use_small { &small } else { &master };
        let mut pixmap = Pixmap::new(size, size).context("pixmap")?;
        render(tree, usvg::FitTo::Size(size, size), pixmap.as_mut()).context("render")?;
        let png_path = out_png.join(format!("mouser_{}.png", size));
        pixmap.save_png(&png_path).with_context(|| format!("save png {size}"))?;
        // hicolor
        let hdir = out_hicolor.join(format!("{}x{}/apps", size, size));
        create_dir_all(&hdir)?;
        let hpath = hdir.join("mouser.png");
        std::fs::copy(&png_path, &hpath)?;
    }

    // Create .ico with common sizes
    let mut icon_dir = IconDir::new(ico::ResourceType::Icon);
    for &size in &[16, 24, 32, 48, 64, 128, 256] {
        let png_path = out_png.join(format!("mouser_{}.png", size));
        let img = image::load_from_memory(&std::fs::read(&png_path)?).context("load for ico")?.to_rgba8();
        let icon_img = IconImage::from_rgba(img.width() as u32, img.height() as u32, img.into_raw()).context("ico img")?;
        icon_dir.add_entry(ico::IconDirEntry::encode(&icon_img).context("encode ico")?);
    }
    let mut ico_file = File::create(out_win.join("mouser.ico"))?;
    icon_dir.write(&mut ico_file)?;

    // Create .icns with Apple icon types
    let mut family = IconFamily::new();
    for &size in &[16, 32, 128, 256, 512, 1024] {
        let png_path = out_png.join(format!("mouser_{}.png", size));
        let data = std::fs::read(&png_path)?;
        let img = IcnsImage::from_png(&data).context("icns from png")?;
        family.add_image(&img).ok(); // ignore unsupported sizes
    }
    let mut icns_file = File::create(out_mac.join("mouser.icns"))?;
    family.write(&mut icns_file).context("write icns")?;

    println!("Generated icons:");
    println!("  SVG master: {}", master_svg.display());
    println!("  PNG 512: {}", out_png.join("mouser_512.png").display());
    println!("  ICO: {}", out_win.join("mouser.ico").display());
    println!("  ICNS: {}", out_mac.join("mouser.icns").display());

    Ok(())
}

fn load_svg(path: &Path, opt: &Options) -> Result<Tree> {
    let data = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let tree = usvg::Tree::from_data(&data, opt).with_context(|| format!("parse svg {}", path.display()))?;
    Ok(tree)
}

