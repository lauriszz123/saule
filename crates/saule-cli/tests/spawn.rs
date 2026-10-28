//! `Os.spawn` on whatever platform this is running on.
//!
//! The fixture in `tests/spawn.sau` covers the same ground in more detail,
//! but it reaches for `sh`, `cat` and `/dev/zero` — so it only says anything
//! on a Unix. These tests exist to pin the behaviour **everywhere**, which
//! means the child cannot be a system command: `cat` is not on Windows,
//! `cmd /c` is not anywhere else, and the two do not agree about quoting.
//!
//! So the child is the `saule` binary itself, running a helper script. It is
//! present by definition whenever these tests run, it behaves identically on
//! every platform, and a helper can be written to do exactly what a case
//! needs — echo its stdin, emit bytes that are not text, sleep, exit with a
//! particular code, report its own directory or environment.

use std::path::{Path, PathBuf};
use std::process::Command;

/// A directory of this test's own, for the helper scripts it writes.
fn scratch(test: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("spawn-tests")
        .join(test);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create the scratch directory");
    dir
}

/// A Saule string literal holding `path`.
///
/// Windows paths are full of backslashes and a Saule string takes those as
/// escapes, so `C:\tmp\x` would otherwise reach the parser as `C:<TAB>mp<?>`.
fn saule_str(path: &Path) -> String {
    format!("\"{}\"", path.display().to_string().replace('\\', "\\\\"))
}

/// Write `source` into `dir` as `name.sau` and return its path.
fn script(dir: &Path, name: &str, source: &str) -> PathBuf {
    let path = dir.join(format!("{name}.sau"));
    std::fs::write(&path, source).expect("write a script");
    path
}

/// Run a Saule program and return its stdout, asserting it succeeded.
///
/// `source` gets `__SAULE__` and `__HELPER__` substituted: the path of the
/// binary under test and of the helper script, both as Saule string
/// literals, so a case can write `Os.spawn(__SAULE__, {"run", __HELPER__})`
/// and stay readable. Underscored so the names cannot match inside an
/// identifier a script of its own uses, such as `SAULE_SPAWN_TEST`.
fn run(dir: &Path, source: &str, helper: &Path) -> String {
    let saule = env!("CARGO_BIN_EXE_saule");
    let source = source
        .replace("__SAULE__", &saule_str(Path::new(saule)))
        .replace("__HELPER__", &saule_str(helper));
    let main = script(dir, "main", &source);
    let out = Command::new(saule)
        .arg("run")
        .arg(&main)
        .output()
        .expect("run saule");
    assert!(
        out.status.success(),
        "the program should succeed.\n--- stdout ---\n{}\n--- stderr ---\n{}\n--- source ---\n{source}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn a_child_reads_what_we_write_and_we_read_what_it_wrote() {
    let dir = scratch("round_trip");
    // The portable `cat`: everything on stdin, back out on stdout.
    let helper = script(
        &dir,
        "echo",
        r#"class Main
	static fn main()
		local all: bytes? = Io.stdin.readBytes()
		Io.stdout.writeBytes(all ?? Bytes.empty())
		Io.stdout.flush()
	end
end
"#,
    );
    let out = run(
        &dir,
        r#"class Main
	static fn main()
		local p: Process = Os.spawn(__SAULE__, {"run", __HELPER__})!
		println("wrote: " .. p.write(Bytes.fromString("round trip")))
		-- The child reads to end of input, so it needs to see one.
		p.closeStdin()
		println("exit: " .. p.wait())
		println("back: " .. (Bytes.toString(p.read()) ?? "?"))
	end
end
"#,
        &helper,
    );
    assert_eq!(out, "wrote: 10\nexit: 0\nback: round trip\n");
}

#[test]
fn output_arrives_byte_for_byte() {
    let dir = scratch("byte_exact");
    // Newlines next to multi-byte UTF-8, written as bytes. The newlines are
    // the point on Windows: a pipe opened in text mode would turn each `\n`
    // into `\r\n` and the hex below would not match. The high bytes catch
    // anything that round-trips through a lossy string conversion.
    //
    // Why not bytes that are *not* UTF-8 at all, which is what `bytes`
    // exists for: Saule refuses to write those to stdout ("open a file
    // instead"), so no Saule child can produce them. That case is covered
    // on Unix by the `tests/spawn.sau` fixture, which has `printf`.
    let helper = script(
        &dir,
        "exact",
        r#"class Main
	static fn main()
		-- "a\n\u{00e9}\n\u{4e16}" — 0x61 0x0a 0xc3 0xa9 0x0a 0xe4 0xb8 0x96
		Io.stdout.writeBytes(Bytes.of(0x61, 0x0a, 0xc3, 0xa9, 0x0a, 0xe4, 0xb8, 0x96))
		Io.stdout.flush()
	end
end
"#,
    );
    let out = run(
        &dir,
        r#"class Main
	static fn main()
		local p: Process = Os.spawn(__SAULE__, {"run", __HELPER__})!
		p.wait()
		local raw: bytes = p.read()
		println("hex: " .. Bytes.toHex(raw))
		println("len: " .. #raw)
	end
end
"#,
        &helper,
    );
    assert_eq!(
        out,
        "hex: 610ac3a90ae4b896
len: 8
",
        "output should arrive byte for byte, with no newline translation"
    );
}

/// True non-UTF-8 output, which needs a child that is not a Saule program.
#[test]
#[cfg(unix)]
fn output_that_is_not_text_survives_intact() {
    let dir = scratch("binary");
    let helper = script(
        &dir,
        "unused",
        "class Main\n\tstatic fn main()\n\tend\nend\n",
    );
    let out = run(
        &dir,
        r#"class Main
	static fn main()
		local p: Process = Os.spawn("printf", {"\\x00\\x80\\xc3\\x28\\xff"})!
		p.wait()
		local raw: bytes = p.read()
		println("hex: " .. Bytes.toHex(raw))
		println("not text: " .. tostring(Bytes.toString(raw) == nil))
	end
end
"#,
        &helper,
    );
    assert_eq!(
        out,
        "hex: 0080c328ff
not text: true
"
    );
}

#[test]
fn reads_do_not_block_while_a_child_is_still_working() {
    let dir = scratch("nonblocking");
    // Output spread over time, so a blocking read would stall the loop.
    let helper = script(
        &dir,
        "slow",
        r#"class Main
	static fn main()
		for i = 1, 3 do
			println("line" .. i)
			Io.stdout.flush()
			Os.sleep(0.05)
		end
	end
end
"#,
    );
    let out = run(
        &dir,
        r#"class Main
	static fn main()
		local p: Process = Os.spawn(__SAULE__, {"run", __HELPER__})!
		local text: string = ""
		local reads: integer = 0
		local deadline: float = Os.clock() + 60.0
		while Os.clock() < deadline do
			reads = reads + 1
			local chunk: bytes = p.read()
			if #chunk > 0 then
				text = text .. (Bytes.toString(chunk) ?? "")
			end
			if p.poll() != nil and p.atEof() then
				break
			end
		end
		println("collected: " .. String.replace(String.trim(text), "\r\n", "\n"))
		println("looped while it worked: " .. tostring(reads > 1))
	end
end
"#,
        &helper,
    );
    // The lines must all arrive, in order; the loop must have gone round
    // more than once, which a blocking read would not have allowed.
    assert!(
        out.contains("collected: line1\nline2\nline3"),
        "expected all three lines in order, got: {out:?}"
    );
    assert!(
        out.contains("looped while it worked: true"),
        "reads should not have blocked, got: {out:?}"
    );
}

#[test]
fn the_two_output_streams_stay_separate() {
    let dir = scratch("streams");
    let helper = script(
        &dir,
        "both",
        r#"class Main
	static fn main()
		Io.stdout.write("to-out")
		Io.stdout.flush()
		Io.stderr.write("to-err")
		Io.stderr.flush()
	end
end
"#,
    );
    let out = run(
        &dir,
        r#"class Main
	static fn main()
		local p: Process = Os.spawn(__SAULE__, {"run", __HELPER__})!
		p.wait()
		println("out: " .. String.trim(Bytes.toString(p.read()) ?? "?"))
		println("err: " .. String.trim(Bytes.toString(p.readErr()) ?? "?"))
	end
end
"#,
        &helper,
    );
    assert_eq!(out, "out: to-out\nerr: to-err\n");
}

#[test]
fn an_exit_code_comes_back_and_is_remembered() {
    let dir = scratch("exit_code");
    let helper = script(
        &dir,
        "exit7",
        r#"class Main
	static fn main()
		Os.exit(7)
	end
end
"#,
    );
    let out = run(
        &dir,
        r#"class Main
	static fn main()
		local p: Process = Os.spawn(__SAULE__, {"run", __HELPER__})!
		println("wait: " .. p.wait())
		-- Asking again is allowed: the code is remembered, because a child
		-- can only be waited on once.
		println("poll: " .. (p.poll() ?? -1))
		println("wait again: " .. p.wait())
		println("running: " .. tostring(p.running()))
	end
end
"#,
        &helper,
    );
    assert_eq!(out, "wait: 7\npoll: 7\nwait again: 7\nrunning: false\n");
}

#[test]
fn a_child_can_be_ended_early() {
    let dir = scratch("kill");
    let helper = script(
        &dir,
        "sleep",
        r#"class Main
	static fn main()
		Os.sleep(120.0)
	end
end
"#,
    );
    let out = run(
        &dir,
        r#"class Main
	static fn main()
		local p: Process = Os.spawn(__SAULE__, {"run", __HELPER__})!
		println("running: " .. tostring(p.running()))
		println("killed: " .. tostring(p.kill()))
		p.wait()
		println("running after: " .. tostring(p.running()))
		-- Already gone, so there is no signal to deliver and that is not an
		-- error — it is the state the caller wanted.
		println("kill again: " .. tostring(p.kill()))
	end
end
"#,
        &helper,
    );
    assert_eq!(
        out,
        "running: true\nkilled: true\nrunning after: false\nkill again: false\n"
    );
}

#[test]
fn cwd_sets_where_the_child_runs() {
    let dir = scratch("cwd");
    let helper = script(
        &dir,
        "pwd",
        r#"class Main
	static fn main()
		println(Os.cwd())
	end
end
"#,
    );
    // A directory of its own, so the answer cannot be the one the test
    // runner happened to start in.
    let inner = dir.join("inner");
    std::fs::create_dir_all(&inner).expect("create the inner directory");
    let source = format!(
        r#"class Main
	static fn main()
		local p: Process = Os.spawn(__SAULE__, {{"run", __HELPER__}}, {})!
		p.wait()
		println("got: " .. String.trim(Bytes.toString(p.read()) ?? "?"))
	end
end
"#,
        saule_str(&inner)
    );
    let out = run(&dir, &source, &helper);
    // Compared canonically: a platform may hand back a resolved path (on
    // macOS `/var/...` for `/private/var/...`), which is the same directory
    // written another way.
    let reported = out
        .trim()
        .strip_prefix("got: ")
        .expect("the program should report a directory");
    let want = inner.canonicalize().expect("canonicalise the directory");
    let got = PathBuf::from(reported)
        .canonicalize()
        .expect("canonicalise what the child reported");
    assert_eq!(got, want, "child ran in the wrong directory: {out:?}");
}

#[test]
fn env_overlays_rather_than_replaces_the_environment() {
    let dir = scratch("env");
    let helper = script(
        &dir,
        "env",
        r#"class Main
	static fn main()
		println("set: " .. (Os.getenv("SAULE_SPAWN_TEST") ?? "<unset>"))
		-- Something the parent's environment always has. If `env` replaced
		-- the environment instead of adding to it, this would be missing.
		local inherited: boolean = Os.getenv("PATH") != nil or Os.getenv("Path") != nil
		println("inherited: " .. tostring(inherited))
	end
end
"#,
    );
    let out = run(
        &dir,
        r#"class Main
	static fn main()
		local p: Process = Os.spawn(__SAULE__, {"run", __HELPER__}, nil, {SAULE_SPAWN_TEST: "overlaid"})!
		p.wait()
		print(Bytes.toString(p.read()) ?? "?")
	end
end
"#,
        &helper,
    );
    assert!(
        out.contains("set: overlaid"),
        "the variable should reach the child, got: {out:?}"
    );
    assert!(
        out.contains("inherited: true"),
        "the rest of the environment should survive, got: {out:?}"
    );
}

#[test]
fn a_program_that_is_not_there_is_nil_rather_than_an_error() {
    let dir = scratch("missing");
    let helper = script(
        &dir,
        "unused",
        "class Main\n\tstatic fn main()\n\tend\nend\n",
    );
    let out = run(
        &dir,
        r#"class Main
	static fn main()
		println("missing: " .. tostring(Os.spawn("saule-no-such-program-xyz") == nil))
		-- A directory that does not exist is the same kind of failure: the
		-- child never starts, so there is no handle to hand back.
		println("bad cwd: " .. tostring(Os.spawn(__SAULE__, {"--version"}, "no-such-dir-xyz") == nil))
	end
end
"#,
        &helper,
    );
    assert_eq!(out, "missing: true\nbad cwd: true\n");
}

#[test]
fn unread_output_is_bounded_and_nothing_is_dropped() {
    let dir = scratch("backpressure");
    // Comfortably more than the 8 MiB a stream will hold unread, so the
    // reader has to stop and resume rather than buffer it all.
    let helper = script(
        &dir,
        "flood",
        r#"class Main
	static fn main()
		local chunk: bytes = Bytes.fromString(String.rep("x", 65536))
		for _ = 1, 192 do
			Io.stdout.writeBytes(chunk)
		end
		Io.stdout.flush()
	end
end
"#,
    );
    let out = run(
        &dir,
        r#"class Main
	static fn main()
		-- Never read: the buffer must stop growing well short of the 12 MiB
		-- the child wants to write.
		local idle: Process = Os.spawn(__SAULE__, {"run", __HELPER__})!
		Os.sleep(2.0)
		local held: integer = idle.available()
		println("bounded: " .. tostring(held > 0 and held <= 8388608))
		idle.kill()

		-- Drained in a loop: every byte must arrive, because backpressure
		-- leaves them in the pipe rather than dropping them.
		local p: Process = Os.spawn(__SAULE__, {"run", __HELPER__})!
		local total: integer = 0
		local deadline: float = Os.clock() + 300.0
		while Os.clock() < deadline do
			total = total + #p.read(1048576)
			if p.poll() != nil and p.atEof() then
				break
			end
		end
		println("all bytes arrived: " .. tostring(total == 192 * 65536))
	end
end
"#,
        &helper,
    );
    assert_eq!(out, "bounded: true\nall bytes arrived: true\n");
}

#[test]
fn many_children_in_a_row_do_not_exhaust_anything() {
    let dir = scratch("churn");
    // Every child costs three pipes and two reader threads. If any of that
    // leaked, the descriptors would run out and `Os.spawn` would start
    // handing back `nil` — which is what makes this a portable leak test,
    // where reading `/proc` would not be. 300 rounds is past the usual
    // 1024-descriptor soft limit at even one leaked pipe per round.
    let helper = script(
        &dir,
        "quick",
        r#"class Main
	static fn main()
		local line: bytes? = Io.stdin.readBytes()
		Io.stdout.write("ok" .. #(line ?? Bytes.empty()))
		Io.stdout.flush()
	end
end
"#,
    );
    let out = run(
        &dir,
        r#"class Main
	static fn main()
		local rounds: integer = 300
		local ok: integer = 0
		local replies: integer = 0
		for _ = 1, rounds do
			local spawned: Process? = Os.spawn(__SAULE__, {"run", __HELPER__})
			if spawned == nil then
				-- Stop at the first failure: the count below names it.
				break
			end
			local p: Process = spawned!
			ok = ok + 1
			-- Use all three streams, so nothing is left untouched.
			p.write(Bytes.fromString("x"))
			p.closeStdin()
			p.wait()
			if #p.read() > 0 then
				replies = replies + 1
			end
		end
		println("spawned: " .. ok .. "/" .. rounds)
		println("answered: " .. replies .. "/" .. rounds)
	end
end
"#,
        &helper,
    );
    assert_eq!(
        out, "spawned: 300/300\nanswered: 300/300\n",
        "every child should start and answer; a shortfall means a descriptor \
         or thread leak per spawn"
    );
}
