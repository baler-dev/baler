//! `export_install_dir` changes this process's environment, so every check
//! lives in one test: tests in a file run in parallel.

use baler_core::export_install_dir;

#[test]
fn the_install_dir_is_exported_as_an_absolute_path() {
    let previous = std::env::var("BALER_LIB").ok();

    // a relative BALER_LIB is made absolute
    unsafe { std::env::set_var("BALER_LIB", "relative/modules") };
    let dir = export_install_dir().unwrap();
    assert!(dir.is_absolute(), "{}", dir.display());
    assert!(dir.ends_with("relative/modules"), "{}", dir.display());
    assert_eq!(std::env::var("BALER_LIB").unwrap(), dir.to_string_lossy());

    // an unset BALER_LIB is filled in with the default
    unsafe { std::env::remove_var("BALER_LIB") };
    let dir = export_install_dir().unwrap();
    assert!(dir.ends_with(".baler/modules") || dir.ends_with(r".baler\modules"), "{}", dir.display());
    assert_eq!(std::env::var("BALER_LIB").unwrap(), dir.to_string_lossy());

    // a child process gets the value, which is how a Makevars sees it
    #[cfg(unix)]
    {
        let shown = std::process::Command::new("sh")
            .args(["-c", "printf %s \"$BALER_LIB\""])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8(shown.stdout).unwrap(), dir.to_string_lossy());
    }

    match previous {
        Some(value) => unsafe { std::env::set_var("BALER_LIB", value) },
        None => unsafe { std::env::remove_var("BALER_LIB") },
    }
}
