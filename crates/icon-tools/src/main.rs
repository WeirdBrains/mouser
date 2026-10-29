use anyhow::{Context, Result};
use icns::{IconFamily, PixelFormat};
use ico::{IconDir, IconImage};
use std::fs::{File, create_dir_all};
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
        let mut pm = pixmap.as_mut();
        let sz = tree.size();
        let sx = size as f32 / sz.width();
        let sy = size as f32 / sz.height();
        let ts = tiny_skia::Transform::from_scale(sx, sy);
        resvg::render(tree, ts, &mut pm);
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
        let icon_img = IconImage::from_rgba_data(img.width() as u32, img.height() as u32, img.into_raw());
        icon_dir.add_entry(ico::IconDirEntry::encode(&icon_img).context("encode ico")?);
    }
    let mut ico_file = File::create(out_win.join("mouser.ico"))?;
    icon_dir.write(&mut ico_file)?;

    // Create .icns with Apple icon types
    let mut family = IconFamily::new();
    for &size in &[16, 32, 128, 256, 512, 1024] {
        let png_path = out_png.join(format!("mouser_{}.png", size));
        let rgba = image::load_from_memory(&std::fs::read(&png_path)?).context("icns load png")?.to_rgba8();
        let (w, h) = rgba.dimensions();
        if let Ok(img) = icns::Image::from_data(PixelFormat::RGBA, w, h, rgba.into_raw()) {
            let _ = family.add_icon(&img);
        }
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
    let mut fontdb = usvg::fontdb::Database::new();
    fontdb.load_system_fonts();
    let tree = usvg::Tree::from_data(&data, opt, &fontdb).with_context(|| format!("parse svg {}", path.display()))?;
    Ok(tree)
}
