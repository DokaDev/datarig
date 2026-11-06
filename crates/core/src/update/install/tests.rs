use super::*;

fn detect(exe: &str, cargo_home: Option<&str>) -> Install {
    Install::detect(Path::new(exe), cargo_home.map(Path::new))
}

#[cfg(unix)]
#[test]
fn the_executable_s_folder_names_the_install() {
    let home = Some("/home/u/.cargo");
    for (exe, want) in [
        ("/opt/homebrew/Cellar/datarig/0.39.0/bin/datarig", Install::Homebrew),
        ("/usr/local/Cellar/datarig/0.39.0/bin/datarig", Install::Homebrew),
        ("/home/linuxbrew/.linuxbrew/Cellar/datarig/0.39.0/bin/datarig", Install::Homebrew),
        ("/srv/brew/Cellar/datarig/0.39.0/bin/datarig", Install::Homebrew),
        ("/home/u/.cargo/bin/datarig", Install::Source),
        ("/home/u/src/datarig/target/release/datarig", Install::Source),
        ("/home/u/src/datarig/target/debug/datarig", Install::Source),
        ("/usr/local/bin/datarig", Install::Other),
        ("/opt/homebrew/bin/datarig", Install::Other),
        ("/home/u/Downloads/datarig-0.39.0/datarig", Install::Other),
        ("/opt/Cellar/other/1.0/bin/datarig", Install::Other),
        ("/home/u/.cargo/bin/sub/datarig", Install::Other),
        ("/home/u/target/datarig", Install::Other),
        ("/home/u/Cellar", Install::Other),
    ] {
        assert_eq!(detect(exe, home), want, "{exe}");
    }
    assert_eq!(detect("/home/u/.cargo/bin/datarig", None), Install::Other);
    assert_eq!(detect("/c/bin/datarig", Some("/c")), Install::Source);
}

#[cfg(unix)]
#[test]
fn a_link_into_the_cellar_is_homebrew() {
    let d = std::env::temp_dir().join(format!("datarig-install-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    let real = d.join("Cellar").join("datarig").join("0.39.0").join("bin");
    std::fs::create_dir_all(&real).unwrap();
    std::fs::create_dir_all(d.join("bin")).unwrap();
    std::fs::write(real.join("datarig"), "").unwrap();
    std::os::unix::fs::symlink(real.join("datarig"), d.join("bin").join("datarig")).unwrap();
    let exe = std::fs::canonicalize(d.join("bin").join("datarig")).unwrap();
    assert_eq!(Install::detect(&exe, None), Install::Homebrew);
    let _ = std::fs::remove_dir_all(&d);
}

#[cfg(windows)]
#[test]
fn windows_paths() {
    assert_eq!(detect(r"C:\Users\u\.cargo\bin\datarig.exe", Some(r"C:\Users\u\.cargo")), Install::Source);
    assert_eq!(detect(r"C:\src\datarig\target\release\datarig.exe", None), Install::Source);
    assert_eq!(detect(r"C:\Tools\datarig\datarig.exe", Some(r"C:\Users\u\.cargo")), Install::Other);
}

#[test]
fn cargo_s_home_is_cargo_home_else_in_the_home_folder() {
    let p = |s: &str| Some(PathBuf::from(s));
    assert_eq!(cargo_home(p("/c"), p("/h")), p("/c"));
    assert_eq!(cargo_home(p(""), p("/h")), Some(Path::new("/h").join(".cargo")));
    assert_eq!(cargo_home(None, p("")), None);
    assert_eq!(cargo_home(None, None), None);
}

#[test]
fn the_running_test_is_built_from_the_source() {
    // `cargo test` runs from target/debug/deps.
    assert_eq!(Install::current(None), Install::Source);
}
