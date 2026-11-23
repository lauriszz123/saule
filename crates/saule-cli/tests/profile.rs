//! `saule run --profile`: the report names the functions the time went to.

use std::process::Command;

#[test]
fn the_profile_names_the_busy_function_and_the_blocking_native() {
    let dir = std::env::temp_dir().join(format!("saule-profile-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let script = dir.join("busy.sau");
    std::fs::write(
        &script,
        "fn busy()\n  local s = 0\n  for i = 1, 3000000 do\n    s = s + i % 7\n  end\nend\n\nbusy()\nOs.sleep(0.2)\n",
    )
    .unwrap();
    let report = dir.join("profile.txt");

    let out = Command::new(env!("CARGO_BIN_EXE_saule"))
        .arg("run")
        .arg(format!("--profile={}", report.display()))
        .arg(&script)
        .output()
        .expect("run saule");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let text = std::fs::read_to_string(&report).expect("the report was written");
    assert!(text.starts_with("profile: "), "{text}");
    // The sleep is time inside a native, charged to it rather than lost.
    assert!(text.contains("Os.sleep (native)"), "{text}");
    assert!(
        text.lines()
            .any(|l| l.contains("  busy  ") && l.contains("busy.sau:")),
        "{text}"
    );
    std::fs::remove_dir_all(&dir).ok();
}
