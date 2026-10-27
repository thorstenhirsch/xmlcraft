//! `cargo xtask fuzz`: a mutation fuzzer that runs on stable Rust (M1.16).
//!
//! Seeds are small XML documents (synthetic ones, plus the files of the W3C suite under 16 KB when `conformance/xmlconf` is fetched).
//! Each iteration mutates a seed, writes it to `fuzz-out/work/`, and runs `xmlcraft-cli check-one <file> --edit` in a child process
//! with a timeout. That opens the bytes as a document, saves them again, analyses, formats, and edits the text every way, holding
//! the engine to its promises (`xmlcraft_engine::selfcheck`).
//!
//! A child that dies (a panic, an abort, a stack overflow, a signal) or fails a promise (exit code 1) is a **crash**, and one that runs
//! past the timeout is a **hang**. Each finding is minimised (chunks are removed while it still fails the same way) and kept in
//! `fuzz-out/findings/` with a JSON note. `fuzz-out/` is git-ignored; findings derived from corpus files must not be committed. Turn
//! each real bug into a synthetic regression test instead.
//!
//! ```text
//! cargo xtask fuzz [--time 300] [--iterations N] [--seed 1] [--jobs 8] [--timeout 10]
//! ```
//! It exits non-zero when it finds a crash (hangs are reported but do not fail the run).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use anyhow::{Result, bail};

use crate::gates::root;

fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).map(String::as_str)
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// xorshift64*: small, deterministic, good enough for mutation choices.
#[derive(Clone)]
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { usize::try_from(self.next() % n as u64).unwrap_or(0) }
    }
}

/// Bytes that matter to XML syntax.
const SYNTAX: &[u8] = b"<>/&;\"'=![]?-: \n\r\t#xX0123456789";

/// Tokens worth splicing in.
const TOKENS: &[&[u8]] = &[
    b"<",
    b">",
    b"</",
    b"/>",
    b"<a>",
    b"</a>",
    b"<a/>",
    b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>",
    b"<?xml version=\"1.0\" encoding=\"UTF-16\"?>",
    b"<?xml version=\"1.1\" standalone=\"yes\"?>",
    b"<?pi data?>",
    b"<!--",
    b"-->",
    b"--",
    b"<![CDATA[",
    b"]]>",
    b"<!DOCTYPE r [",
    b"<!DOCTYPE r SYSTEM \"x.dtd\">",
    b"]>",
    b"<!ELEMENT r (a|b)*>",
    b"<!ATTLIST r a CDATA #IMPLIED>",
    b"<!ENTITY e \"&e;\">",
    b"<!ENTITY % p \"x\">",
    b"%p;",
    b"&e;",
    b"&amp;",
    b"&#0;",
    b"&#x10FFFF;",
    b"&#xD800;",
    b"&lt",
    b" xmlns=\"urn:x\"",
    b" xmlns:p=\"urn:p\"",
    b" p:q=\"1\"",
    b" xml:space=\"preserve\"",
    b" a=\"1\" a=\"2\"",
    b"\xEF\xBB\xBF",
    b"\xFF\xFE",
    b"\xFE\xFF",
    b"\xC3",
    b"\xE6\x97\xA5",
    b"\xF0\x9F\x98\x80",
    b"\xED\xA0\x80",
    b"\r\n",
    b"\r",
    b"\0",
];

fn mutate(rng: &mut Rng, seed: &[u8], other: &[u8]) -> Vec<u8> {
    let mut d = seed.to_vec();
    let rounds = 1 + rng.below(4);
    for _ in 0..rounds {
        if d.is_empty() {
            d.extend_from_slice(b"<a/>");
        }
        let at = rng.below(d.len());
        match rng.below(8) {
            0 => {
                if let Some(byte) = d.get_mut(at) {
                    *byte ^= 1 << rng.below(8);
                }
            }
            1 => {
                if let Some(byte) = d.get_mut(at) {
                    *byte = SYNTAX.get(rng.below(SYNTAX.len())).copied().unwrap_or(b'<');
                }
            }
            2 => {
                let n = 1 + rng.below(64.min(d.len() - at));
                d.drain(at..at + n);
            }
            3 => {
                let n = 1 + rng.below(256.min(d.len() - at));
                let chunk = d.get(at..at + n).map(<[u8]>::to_vec).unwrap_or_default();
                let to = rng.below(d.len());
                d.splice(to..to, chunk);
            }
            4 => {
                let token = TOKENS.get(rng.below(TOKENS.len())).copied().unwrap_or(b"<");
                d.splice(at..at, token.iter().copied());
            }
            5 => d.truncate(at.max(1)),
            6 if !other.is_empty() => {
                let from = rng.below(other.len());
                let n = 1 + rng.below(512.min(other.len() - from));
                d.splice(at..at, other.get(from..from + n).unwrap_or_default().iter().copied());
            }
            _ => {
                // Replace a run of digits with an extreme number (character references, sizes, versions).
                if let Some(start) = d.get(at..).and_then(|rest| rest.iter().position(u8::is_ascii_digit)).map(|p| p + at) {
                    let end = d.get(start..).and_then(|rest| rest.iter().position(|c| !c.is_ascii_digit())).map_or(d.len(), |p| p + start);
                    let value: &[u8] = [&b"0"[..], b"4294967295", b"9223372036854775807", b"1114112", b"55296"][rng.below(5)];
                    d.splice(start..end, value.iter().copied());
                }
            }
        }
    }
    d
}

/// Synthetic seeds, so fuzzing works without the suite: every kind of markup, line endings and encodings, a DTD, namespaces, mixed
/// content, `xml:space`, long and deep documents.
fn synthetic_seeds() -> Vec<Vec<u8>> {
    let deep: String = (0..80).map(|i| format!("<e{i}>")).collect::<String>() + "x" + &(0..80).rev().map(|i| format!("</e{i}>")).collect::<String>();
    let wide: String = (0..300).map(|i| format!("  <row id=\"{i}\" k=\"{}\"><v>{i}</v></row>\n", i % 7)).collect::<String>();
    let texts: Vec<String> = vec![
        "<a/>".into(),
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!-- c -->\n<a b=\"1\" c='2'>\n  <d>text &amp; &#65; &#x42; more</d>\n  <![CDATA[ <raw> & ]]>\n  <?pi data?>\n</a>\n".into(),
        "<a>\r\n  <b>\r\n    <c/>\r\n  </b>\r\n</a>\r\n".into(),
        "<!DOCTYPE r [\n<!ELEMENT r (a|b)*>\n<!ATTLIST r id ID #IMPLIED>\n<!ENTITY e \"value\">\n<!ENTITY % p \"<!ENTITY q 'x'>\">\n%p;\n]>\n<r id=\"1\">&e;<a/><b/></r>\n".into(),
        "<r:root xmlns:r=\"urn:r\" xmlns=\"urn:d\" r:a=\"1\"><child r:b=\"2\"><r:x/></child></r:root>".into(),
        "<p xml:space=\"preserve\">  keep   <b>this</b>   and <i>that</i>  </p>\n<q>mixed <b>content</b> here</q>".into(),
        "<a><b></a>".into(),
        "<a b=\"1>text</a>".into(),
        "<!-- unclosed".into(),
        "<![CDATA[ unclosed".into(),
        "<a>é日\u{1F600}</a>".into(),
        deep,
        format!("<table>\n{wide}</table>\n"),
    ];
    let mut out: Vec<Vec<u8>> = texts.into_iter().map(String::into_bytes).collect();
    out.push(b"\xEF\xBB\xBF<a>\xC3\xA9</a>".to_vec());
    out.push(b"\xFF\xFE<\0a\0>\0\xE9\0<\0/\0a\0>\0".to_vec());
    out.push(b"<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?><a>\xE9\xFC</a>".to_vec());
    out.push(b"<a>\xFF\xFF</a>".to_vec());
    out
}

fn seeds() -> Vec<(String, Vec<u8>)> {
    let mut out: Vec<(String, Vec<u8>)> = synthetic_seeds().into_iter().enumerate().map(|(i, b)| (format!("synthetic-{i}"), b)).collect();
    // The W3C suite, if fetched: small files, in a fixed order so a seed number means the same file on every run.
    let mut files = Vec::new();
    collect_xml(&root().join("conformance/xmlconf"), 0, &mut files);
    files.sort();
    for file in files {
        if let Ok(bytes) = std::fs::read(&file)
            && bytes.len() <= 16 * 1024
        {
            out.push((file.file_name().unwrap_or_default().to_string_lossy().into_owned(), bytes));
        }
    }
    out
}

fn collect_xml(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > 6 || out.len() > 5000 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_xml(&path, depth + 1, out);
        } else if path.extension().is_some_and(|e| e == "xml") {
            out.push(path);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Outcome {
    Ok,
    Crash,
    Hang,
}

/// Address-space cap for each child on Linux, in KiB (4 GiB). An input that makes us allocate without bound then fails its allocation in
/// the child (an abort, kept as a crash finding) instead of exhausting the machine.
const CHILD_ADDRESS_SPACE_KIB: u64 = 4 * 1024 * 1024;

/// The child process, under the address-space cap where the shell can set one (Linux; macOS does not support `ulimit -v` and Windows
/// has no `sh`).
fn child_command(exe: &Path) -> Command {
    if cfg!(target_os = "linux") {
        let mut c = Command::new("sh");
        c.args(["-c", &format!("ulimit -v {CHILD_ADDRESS_SPACE_KIB} && exec \"$0\" \"$@\"")]).arg(exe);
        c
    } else {
        Command::new(exe)
    }
}

/// The text that says why a child failed: the first panic location or abort message, or the self-check's own message.
fn reason(stderr: &str) -> String {
    let what = stderr
        .lines()
        .find(|l| {
            l.contains("panicked at")
                || l.contains("overflow")
                || l.contains("fatal")
                || l.contains("memory allocation")
                || l.contains("self-check failed")
        })
        .unwrap_or("")
        .trim();
    // Drop the per-process thread id ("thread 'main' (12345)") so runs compare equal.
    what.split(" (")
        .map(|part| match part.split_once(')') {
            Some((id, rest)) if id.chars().all(|c| c.is_ascii_digit()) => rest.to_string(),
            _ => format!(" ({part}"),
        })
        .collect::<String>()
        .trim_start_matches(" (")
        .to_string()
}

fn run_one(exe: &Path, file: &Path, timeout: Duration) -> (Outcome, String) {
    let child = child_command(exe).args(["check-one", &file.to_string_lossy(), "--edit"]).stdout(Stdio::null()).stderr(Stdio::piped()).spawn();
    let Ok(mut child) = child else { return (Outcome::Ok, String::new()) };
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if status.success() {
                    return (Outcome::Ok, String::new());
                }
                let mut err = String::new();
                if let Some(mut e) = child.stderr.take() {
                    use std::io::Read;
                    let _ = e.read_to_string(&mut err);
                }
                return (Outcome::Crash, format!("{status}; {}", reason(&err)));
            }
            Ok(None) if start.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return (Outcome::Hang, format!("over {} s", timeout.as_secs()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(5)),
            Err(e) => return (Outcome::Ok, e.to_string()),
        }
    }
}

/// Remove chunks while the input still fails with the same outcome and message.
fn minimize(exe: &Path, probe: &Path, data: Vec<u8>, outcome: Outcome, what: &str, timeout: Duration) -> Vec<u8> {
    let mut best = data;
    let mut chunk = best.len() / 2;
    let mut tries = 0;
    while chunk >= 1 && tries < 300 {
        let mut i = 0;
        let mut progressed = false;
        while i < best.len() && tries < 300 {
            let mut candidate = best.clone();
            candidate.drain(i..(i + chunk).min(candidate.len()));
            tries += 1;
            if std::fs::write(probe, &candidate).is_err() {
                break;
            }
            let (o, w) = run_one(exe, probe, timeout);
            if o == outcome && (outcome == Outcome::Hang || w == what) {
                best = candidate;
                progressed = true;
            } else {
                i += chunk;
            }
        }
        if !progressed {
            chunk /= 2;
        }
    }
    best
}

fn cli_path() -> PathBuf {
    let target = std::env::var_os("CARGO_TARGET_DIR").map(PathBuf::from).unwrap_or_else(|| root().join("target"));
    target.join("release").join(if cfg!(windows) { "xmlcraft-cli.exe" } else { "xmlcraft-cli" })
}

pub fn run(args: &[String]) -> Result<()> {
    let time = Duration::from_secs(flag(args, "--time").unwrap_or("300").parse()?);
    let iterations: usize = flag(args, "--iterations").map(str::parse).transpose()?.unwrap_or(usize::MAX);
    let seed: u64 = flag(args, "--seed").unwrap_or("1").parse()?;
    // Half the cores by default: a saturated machine turns slow inputs into false hangs.
    let jobs: usize =
        flag(args, "--jobs").map(str::parse).transpose()?.unwrap_or_else(|| std::thread::available_parallelism().map_or(2, |n| (n.get() / 2).max(1)));
    let timeout = Duration::from_secs(flag(args, "--timeout").unwrap_or("10").parse()?);

    let status = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .current_dir(root())
        .args(["build", "--release", "-q", "-p", "xmlcraft-cli"])
        .status()?;
    if !status.success() {
        bail!("building xmlcraft-cli failed");
    }
    let exe = cli_path();
    if !exe.is_file() {
        // Every run would fail to start and count as a pass.
        bail!("{} not found after building it", exe.display());
    }
    let out = root().join("fuzz-out");
    let work = out.join("work");
    let findings = out.join("findings");
    std::fs::create_dir_all(&work)?;
    std::fs::create_dir_all(&findings)?;
    let seeds = Arc::new(seeds());
    println!("fuzz: {} seeds, {jobs} jobs, {} s, timeout {} s, seed {seed}", seeds.len(), time.as_secs(), timeout.as_secs());

    let done = Arc::new(AtomicUsize::new(0));
    let found: Arc<Mutex<Vec<(Outcome, String, String)>>> = Arc::default();
    // Held while confirming a hang; the other jobs keep running, so this only serialises confirmations with each other.
    let solo = Arc::new(Mutex::new(()));
    let start = Instant::now();
    std::thread::scope(|s| {
        for job in 0..jobs {
            let (seeds, done, found, exe, work, findings, solo) =
                (seeds.clone(), done.clone(), found.clone(), exe.clone(), work.clone(), findings.clone(), solo.clone());
            s.spawn(move || {
                let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(job as u64 + 1) | 1);
                let file = work.join(format!("job{job}.xml"));
                // Each job minimises in a file of its own: a shared one would mix up what two jobs are probing.
                let probe = work.join(format!("probe{job}.xml"));
                while start.elapsed() < time {
                    let n = done.fetch_add(1, Ordering::Relaxed);
                    if n >= iterations {
                        break;
                    }
                    let Some((name, base)) = seeds.get(rng.below(seeds.len())) else { break };
                    let other = seeds.get(rng.below(seeds.len())).map(|s| s.1.as_slice()).unwrap_or_default();
                    let data = mutate(&mut rng, base, other);
                    if std::fs::write(&file, &data).is_err() {
                        continue;
                    }
                    let (outcome, what) = run_one(&exe, &file, timeout);
                    if outcome == Outcome::Ok {
                        continue;
                    }
                    // One finding per distinct failure (hangs: one per seed).
                    let key = |o: Outcome, w: &str, s: &str| if o == Outcome::Hang { format!("Hang:{s}") } else { format!("{o:?}:{w}") };
                    if locked(&found).iter().any(|(o, w, s)| key(*o, w, s) == key(outcome, &what, name)) {
                        continue;
                    }
                    // A hang under full load may just be a slow machine: confirm it alone, with three times the time, before recording
                    // it. Hangs are not minimised (each probe costs a full timeout).
                    let small = if outcome == Outcome::Hang {
                        let _guard = locked(&solo);
                        if run_one(&exe, &file, timeout * 3).0 != Outcome::Hang {
                            continue;
                        }
                        data.clone()
                    } else {
                        minimize(&exe, &probe, data.clone(), outcome, &what, timeout)
                    };
                    // Another job may have found the same while this one minimised.
                    {
                        let mut seen = locked(&found);
                        if seen.iter().any(|(o, w, s)| key(*o, w, s) == key(outcome, &what, name)) {
                            continue;
                        }
                        seen.push((outcome, what.clone(), name.clone()));
                    }
                    let id = format!("{:?}-{:016x}", outcome, fxhash(&small)).to_lowercase();
                    let _ = std::fs::write(findings.join(format!("{id}.xml")), &small);
                    let note = serde_json::json!({ "outcome": format!("{outcome:?}"), "detail": what, "seed_file": name, "bytes": small.len(), "original_bytes": data.len() });
                    let _ = std::fs::write(findings.join(format!("{id}.json")), serde_json::to_string_pretty(&note).unwrap_or_default());
                    println!("  {outcome:?} from {name}: {what} → fuzz-out/findings/{id}.xml ({} bytes)", small.len());
                }
            });
        }
    });
    let n = done.load(Ordering::Relaxed).min(iterations);
    let found = locked(&found);
    let crashes = found.iter().filter(|f| f.0 == Outcome::Crash).count();
    let hangs = found.len() - crashes;
    println!(
        "fuzz: {n} runs in {:.0} s ({:.1}/s): {crashes} distinct crash(es), {hangs} hang(s)",
        start.elapsed().as_secs_f64(),
        n as f64 / start.elapsed().as_secs_f64().max(0.001)
    );
    if crashes > 0 {
        bail!("{crashes} crash(es): see fuzz-out/findings/");
    }
    Ok(())
}

fn fxhash(b: &[u8]) -> u64 {
    b.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, x| (h ^ u64::from(*x)).wrapping_mul(0x100_0000_01b3))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mutation_is_deterministic_and_never_panics_on_any_seed_or_empty_input() {
        let seeds = synthetic_seeds();
        for (i, seed) in seeds.iter().chain(std::iter::once(&Vec::new())).enumerate() {
            for round in 0..200u64 {
                let (mut a, mut b) = (Rng(round * 7 + i as u64 + 1), Rng(round * 7 + i as u64 + 1));
                let other = seeds.get((round as usize) % seeds.len()).map(Vec::as_slice).unwrap_or_default();
                assert_eq!(mutate(&mut a, seed, other), mutate(&mut b, seed, other));
            }
        }
        // Nothing to mutate and nothing to take from: a document is made.
        assert!(!mutate(&mut Rng(1), &[], &[]).is_empty());
    }

    #[test]
    fn the_seeds_are_kept_by_the_self_check() {
        // The fuzzer starts from documents the engine keeps every promise for; a failure here is a finding of its own.
        for (i, seed) in synthetic_seeds().iter().enumerate() {
            xmlcraft_engine::selfcheck::check(seed, true).unwrap_or_else(|e| panic!("synthetic seed {i}: {e}"));
        }
    }

    #[test]
    fn the_reason_names_the_panic_or_the_broken_promise_without_the_thread_id() {
        let panic = "thread 'main' (4242) panicked at crates/x/src/y.rs:1:2:\nboom";
        assert_eq!(reason(panic), "thread 'main' panicked at crates/x/src/y.rs:1:2:");
        assert_eq!(reason("xmlcraft-cli: self-check failed: undoing everything\n"), "xmlcraft-cli: self-check failed: undoing everything");
        assert_eq!(reason("nothing of interest"), "");
    }
}
