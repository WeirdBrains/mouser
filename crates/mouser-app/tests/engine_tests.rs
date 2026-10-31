//! Custom engines end to end with the example engine (`flat`): install, list,
//! use, uninstall, ABI checks and the load-failure fallback paths. Every test
//! runs with a temporary MOUSER_HOME.

use mouser_app::engine_cli::{Action, run};
use mouser_core::engines;
use serial_test::serial;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Build the example engine once and return its library.
fn example_lib() -> PathBuf {
    static LIB: OnceLock<PathBuf> = OnceLock::new();
    LIB.get_or_init(|| {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let target = root.join("target/engine-tests");
        let st = std::process::Command::new(env!("CARGO"))
            .args(["build", "-q", "-p", "mouser-engine-example", "--target-dir"])
            .arg(&target)
            .current_dir(&root)
            .status()
            .expect("cargo");
        assert!(st.success(), "building the example engine failed");
        let name = format!("{}mouser_engine_flat{}", std::env::consts::DLL_PREFIX, std::env::consts::DLL_SUFFIX);
        target.join("debug").join(name)
    })
    .clone()
}

struct Home(tempfile::TempDir);

fn home() -> Home {
    let t = tempfile::tempdir().unwrap();
    // SAFETY: serial tests.
    unsafe {
        std::env::set_var("MOUSER_HOME", t.path().join("mouser"));
        std::env::set_var("HOME", t.path());
        std::env::remove_var("MOUSER_ENGINE");
        std::env::remove_var("MOUSER_FLAT_ABI");
        std::env::remove_var("MOUSER_FLAT_FAIL");
    }
    Home(t)
}

/// A source directory like a downloaded engine: engine.toml + library.
fn source_dir(h: &Home) -> PathBuf {
    let d = h.0.path().join("flat-src");
    std::fs::create_dir_all(&d).unwrap();
    let lib = example_lib();
    let lib_name = lib.file_name().unwrap().to_string_lossy().into_owned();
    std::fs::copy(&lib, d.join(&lib_name)).unwrap();
    std::fs::write(
        d.join("engine.toml"),
        format!("name = \"flat\"\nversion = \"0.1.0\"\nabi = 1\nlibrary = \"{lib_name}\"\ndescription = \"example\"\n"),
    )
    .unwrap();
    d
}

fn cli(a: Action) -> anyhow::Result<String> {
    let mut out = Vec::new();
    run(&a, &mut out)?;
    Ok(String::from_utf8(out).unwrap())
}

#[test]
#[serial]
fn install_list_use_uninstall() {
    let h = home();
    let src = source_dir(&h);
    let out = cli(Action::Install(src.display().to_string())).unwrap();
    assert!(out.contains("installed flat 0.1.0"), "{out}");
    let list = cli(Action::List).unwrap();
    assert!(list.contains("* mouser"), "{list}");
    assert!(list.contains("flat"), "{list}");
    cli(Action::Use("flat".into())).unwrap();
    assert_eq!(engines::chosen(), "flat");
    assert!(cli(Action::List).unwrap().contains("* flat"));
    let lib = mouser_render::engine::load_installed("flat").unwrap();
    assert_eq!(lib.name(), "flat");
    drop(lib); // Windows cannot delete a loaded DLL.
    let out = cli(Action::Uninstall("flat".into())).unwrap();
    assert!(out.contains("mouser engine"), "{out}");
    assert_eq!(engines::chosen(), "mouser");
    assert!(cli(Action::Use("flat".into())).is_err());
    assert!(cli(Action::Uninstall("mouser".into())).is_err());
}

#[test]
#[serial]
fn install_rejects_urls_wrong_abi_and_bad_libraries() {
    let h = home();
    assert!(cli(Action::Install("https://example.com/e.tar".into())).unwrap_err().to_string().contains("URL"));
    let src = source_dir(&h);
    // The library itself reports another ABI version.
    unsafe { std::env::set_var("MOUSER_FLAT_ABI", "7") };
    let e = cli(Action::Install(src.display().to_string())).unwrap_err();
    assert!(format!("{e:#}").contains("ABI 7"), "{e:#}");
    unsafe { std::env::remove_var("MOUSER_FLAT_ABI") };
    // Not a library at all (a new file name: loaders cache libraries by path).
    let junk = format!("{}junk{}", std::env::consts::DLL_PREFIX, std::env::consts::DLL_SUFFIX);
    std::fs::write(src.join(&junk), b"garbage").unwrap();
    let toml = std::fs::read_to_string(src.join("engine.toml")).unwrap();
    let real = toml.lines().find(|l| l.starts_with("library")).unwrap().to_string();
    std::fs::write(src.join("engine.toml"), toml.replace(&real, &format!("library = \"{junk}\""))).unwrap();
    assert!(cli(Action::Install(src.display().to_string())).is_err());
    assert!(engines::installed().is_empty());
}

#[test]
#[serial]
fn load_failures_fall_back() {
    let h = home();
    let src = source_dir(&h);
    cli(Action::Install(src.display().to_string())).unwrap();
    cli(Action::Use("flat".into())).unwrap();

    // create() failing reports why (Mouser then uses the mouser engine).
    let lib = mouser_render::engine::load_installed("flat").unwrap();
    let none = mouser_engine_abi::MouserSurface { kind: 0, display: std::ptr::null_mut(), window: std::ptr::null_mut() };
    let e = lib.create_raw(&none, 100, 100).unwrap_err();
    assert!(e.to_string().contains("unsupported surface"), "{e}");
    unsafe { std::env::set_var("MOUSER_FLAT_FAIL", "create") };
    let e = lib.create_raw(&none, 100, 100).unwrap_err();
    assert!(e.to_string().contains("MOUSER_FLAT_FAIL"), "{e}");
    unsafe { std::env::remove_var("MOUSER_FLAT_FAIL") };

    // A crash during the last start (sentinel left behind) skips the engine.
    engines::set_sentinel("flat");
    let e = mouser_render::engine::load_installed("flat").err().unwrap();
    assert!(e.to_string().contains("crashed"), "{e}");
    assert!(cli(Action::List).unwrap().contains("crashed last start"));
    cli(Action::Use("flat".into())).unwrap();
    assert!(mouser_render::engine::load_installed("flat").is_ok());

    // A missing library is a load error, not a crash. (Point the manifest at
    // another file: the dynamic loader keeps the already-loaded one by path.)
    let inst = engines::find("flat").unwrap();
    let toml = std::fs::read_to_string(inst.dir.join("engine.toml")).unwrap();
    let lib_name = inst.manifest.library.clone();
    std::fs::write(inst.dir.join("engine.toml"), toml.replace(&lib_name, "libgone.so")).unwrap();
    assert!(mouser_render::engine::load_installed("flat").is_err());
    // An unknown engine name too.
    assert!(mouser_render::engine::load_installed("nope").is_err());
}
