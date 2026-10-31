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
    assert!(list.lines().any(|l| l.starts_with("* 1") && l.contains("mouser")), "{list}");
    assert!(list.contains("flat"), "{list}");
    cli(Action::Use("flat".into())).unwrap();
    assert_eq!(engines::chosen(), "flat");
    assert!(cli(Action::List).unwrap().lines().any(|l| l.starts_with("* 1") && l.contains("flat")));
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
    assert!(cli(Action::List).unwrap().contains("mouser engine unskip flat"));
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

#[test]
#[serial]
fn list_states_unskip_and_restored_engine_is_used() {
    let h = home();
    let src = source_dir(&h);
    cli(Action::Install(src.display().to_string())).unwrap();
    cli(Action::Use("flat".into())).unwrap();
    let list = cli(Action::List).unwrap();
    assert!(list.lines().any(|l| l.starts_with("* 1") && l.contains("flat") && l.contains(" ok ")), "{list}");

    // Crashed while starting: skipped, with when and the restore command.
    engines::set_sentinel("flat");
    let list = cli(Action::List).unwrap();
    assert!(list.lines().any(|l| l.starts_with("* 1") && l.contains("flat") && l.contains("skipped")), "{list}");
    assert!(list.contains("crashed Mouser while starting") && list.contains("s ago"), "{list}");
    assert!(list.contains("restore: mouser engine unskip flat"), "{list}");
    let e = mouser_render::engine::skip_check("flat").unwrap_err();
    assert!(e.contains("`mouser engine unskip flat`"), "{e}");
    assert!(mouser_render::engine::load_installed("flat").is_err());

    // A failure while starting/drawing is recorded and shown, and still tried.
    let msg = mouser_render::engine::fallback_message("skia", "no GL display", "failed to start");
    assert!(msg.contains("trying the next engine") && msg.contains("mouser engine unskip skia"), "{msg}");
    let list = cli(Action::List).unwrap();
    assert!(list.lines().any(|l| l.contains("skia") && l.contains("failed")), "{list}");
    assert!(list.contains("failed to start: no GL display"), "{list}");
    assert!(mouser_render::engine::skip_check("skia").is_ok());

    // Unskip one: cleared, still selected (it was), used on the next start.
    let out = cli(Action::Unskip(Some("flat".into()))).unwrap();
    assert!(out.contains("no longer skipped"), "{out}");
    assert_eq!(engines::state("flat"), engines::State::Ok);
    assert_eq!(engines::chosen(), "flat");
    assert!(mouser_render::engine::skip_check("flat").is_ok());
    assert_eq!(mouser_render::engine::load_installed("flat").unwrap().name(), "flat");

    // Not skipped / unknown.
    assert!(cli(Action::Unskip(Some("flat".into()))).unwrap().contains("nothing to do"));
    assert!(cli(Action::Unskip(Some("nope".into()))).is_err());

    // Unskip does not select; --all clears everything.
    cli(Action::Use("mouser".into())).unwrap();
    engines::set_sentinel("flat");
    let out = cli(Action::Unskip(None)).unwrap();
    assert!(out.contains("restored: skia, flat"), "{out}");
    assert_eq!(engines::chosen(), "mouser");
    assert!(cli(Action::Unskip(None)).unwrap().contains("nothing to do"));
    let list = cli(Action::List).unwrap();
    assert!(!list.lines().any(|l| l.contains(" skipped ") || l.contains(" failed ")), "{list}");
}

#[test]
#[serial]
fn cli_priority_and_move_keep_mouser() {
    let h = home();
    let src = source_dir(&h);
    cli(Action::Install(src.display().to_string())).unwrap();
    let out = cli(Action::Priority(vec!["flat".into(), "skia".into()])).unwrap();
    assert!(out.contains("flat > skia > mouser"), "{out}");
    let list = cli(Action::List).unwrap();
    let names: Vec<&str> = list.lines().skip(1).filter_map(|l| l.split_whitespace().nth(if l.starts_with('*') { 2 } else { 1 })).take(3).collect();
    assert_eq!(names, ["flat", "skia", "mouser"], "{list}");
    assert!(list.contains("order (preferences): flat > skia > mouser"), "{list}");
    assert!(cli(Action::Move("mouser".into(), "first".into())).unwrap().contains("mouser > flat > skia"));
    assert!(cli(Action::Move("skia".into(), "2".into())).unwrap().contains("mouser > skia > flat"));
    assert!(cli(Action::Move("flat".into(), "up".into())).unwrap().contains("mouser > flat > skia"));
    assert!(cli(Action::Move("mouser".into(), "last".into())).unwrap().contains("flat > skia > mouser"));
    assert!(cli(Action::Move("flat".into(), "zero".into())).is_err());
    assert!(cli(Action::Use("skia".into())).unwrap().contains("skia > flat > mouser"));
    let e = cli(Action::Uninstall("mouser".into())).unwrap_err().to_string();
    assert!(e.contains("never be removed"), "{e}");
    // Fallback walk: the next engine after a failed one, mouser last.
    let order = engines::priority();
    assert_eq!(mouser_render::engine::after(&order, "skia"), ["flat", "mouser"]);
    assert_eq!(mouser_render::engine::after(&order, "mouser"), ["mouser"]);
    // A skipped engine is passed over by the walk and listed as such.
    engines::set_sentinel("skia");
    assert!(mouser_render::engine::skip_check("skia").is_err());
    assert!(mouser_render::engine::skip_check("flat").is_ok());
    // MOUSER_ENGINE overrides the order for one run.
    unsafe { std::env::set_var("MOUSER_ENGINE", "flat") };
    assert!(cli(Action::List).unwrap().contains("order (MOUSER_ENGINE): flat > mouser"));
    unsafe { std::env::remove_var("MOUSER_ENGINE") };
}
