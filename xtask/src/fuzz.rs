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
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use crate::gates::root;

/// What the command line asks for. Every option takes a value, and an unknown, a repeated or a valueless one is an error (a missing
/// `--time` value must not quietly mean five minutes).
#[derive(Debug, PartialEq, Eq)]
struct Options {
    time: Duration,
    iterations: usize,
    seed: u64,
    jobs: usize,
    timeout: Duration,
}

/// The most jobs: each is a process at a time, and a typo should not start thousands of threads.
const MAX_JOBS: usize = 64;

impl Options {
    fn parse(args: &[String]) -> Result<Self> {
        let mut given: std::collections::BTreeMap<&str, &str> = std::collections::BTreeMap::new();
        let mut rest = args.iter();
        while let Some(name) = rest.next() {
            let name = name.as_str();
            if !matches!(name, "--time" | "--iterations" | "--seed" | "--jobs" | "--timeout") {
                bail!("unknown option {name}: fuzz takes [--time S] [--iterations N] [--seed N] [--jobs N] [--timeout S]");
            }
            let Some(value) = rest.next() else { bail!("{name} needs a value") };
            if given.insert(name, value).is_some() {
                bail!("{name} is given twice");
            }
        }
        let number = |name: &str, default: u64, least: u64, most: u64| -> Result<u64> {
            let Some(text) = given.get(name) else { return Ok(default) };
            let n: u64 = text.parse().map_err(|_| anyhow::anyhow!("{name} needs a number, not {text:?}"))?;
            if !(least..=most).contains(&n) {
                bail!("{name} must be between {least} and {most}, not {n}");
            }
            Ok(n)
        };
        // Half the cores by default: a saturated machine turns slow inputs into false hangs.
        let default_jobs = std::thread::available_parallelism().map_or(2, |n| (n.get() / 2).max(1)).min(MAX_JOBS) as u64;
        Ok(Self {
            time: Duration::from_secs(number("--time", 300, 1, 7 * 24 * 3600)?),
            iterations: usize::try_from(number("--iterations", u64::MAX, 1, u64::MAX)?).unwrap_or(usize::MAX),
            seed: number("--seed", 1, 0, u64::MAX)?,
            jobs: usize::try_from(number("--jobs", default_jobs, 1, MAX_JOBS as u64)?).unwrap_or(1),
            timeout: Duration::from_secs(number("--timeout", 10, 1, 3600)?),
        })
    }
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

/// What makes two failures the same bug: a panic is where it happened; a broken promise is which one, whatever offsets, steps and sizes
/// its message names (they change as the input is cut down, and a cut-down input is the point).
fn class(what: &str) -> String {
    if what.contains("panicked at") {
        return what.to_owned();
    }
    let mut out = String::with_capacity(what.len());
    let mut in_number = false;
    for c in what.chars() {
        if c.is_ascii_digit() {
            if !in_number {
                out.push('#');
            }
            in_number = true;
        } else {
            in_number = false;
            out.push(c);
        }
    }
    out
}

/// Running the child went wrong in a way that says nothing about the input (it did not start, it could not be waited for, its work file
/// could not be written): the run stops with this, instead of counting every input as passed.
type Infrastructure = anyhow::Error;

/// Run the child on `file` (its standard error goes to `stderr_file`, so that a child that writes much cannot block on a full pipe and
/// look like a hang).
fn run_one(exe: &Path, file: &Path, stderr_file: &Path, timeout: Duration) -> Result<(Outcome, String), Infrastructure> {
    let stderr = std::fs::File::create(stderr_file).with_context(|| format!("creating {}", stderr_file.display()))?;
    let mut child = child_command(exe)
        .args(["check-one", &file.to_string_lossy(), "--edit"])
        .stdout(Stdio::null())
        .stderr(Stdio::from(stderr))
        .spawn()
        .with_context(|| format!("starting {}", exe.display()))?;
    let start = Instant::now();
    loop {
        match child.try_wait().context("waiting for the child")? {
            Some(status) if status.success() => return Ok((Outcome::Ok, String::new())),
            // 126 and 127 are the shell saying it could not run the program (on Linux the child is started through `sh`): nothing the
            // input did, and `check-one` never exits with them.
            Some(status) if matches!(status.code(), Some(126 | 127)) => {
                let err = std::fs::read(stderr_file).map(|bytes| String::from_utf8_lossy(&bytes).into_owned()).unwrap_or_default();
                bail!("the child could not be run ({status}): {}", err.trim());
            }
            Some(status) => {
                let err = std::fs::read(stderr_file).map(|bytes| String::from_utf8_lossy(&bytes).into_owned()).unwrap_or_default();
                return Ok((Outcome::Crash, format!("{status}; {}", reason(&err))));
            }
            None if start.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Ok((Outcome::Hang, format!("over {} s", timeout.as_secs())));
            }
            None => std::thread::sleep(Duration::from_millis(5)),
        }
    }
}

/// Remove chunks while the input still fails the same way (the same class of failure).
fn minimize(
    exe: &Path,
    probe: &Path,
    stderr_file: &Path,
    data: Vec<u8>,
    outcome: Outcome,
    what: &str,
    timeout: Duration,
) -> Result<Vec<u8>, Infrastructure> {
    let wanted = class(what);
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
            std::fs::write(probe, &candidate).with_context(|| format!("writing {}", probe.display()))?;
            let (o, w) = run_one(exe, probe, stderr_file, timeout)?;
            if o == outcome && (outcome == Outcome::Hang || class(&w) == wanted) {
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
    Ok(best)
}

fn cli_path() -> PathBuf {
    let target = std::env::var_os("CARGO_TARGET_DIR").map(PathBuf::from).unwrap_or_else(|| root().join("target"));
    target.join("release").join(if cfg!(windows) { "xmlcraft-cli.exe" } else { "xmlcraft-cli" })
}

/// One distinct failure: hangs are told apart by the seed they came from, crashes by their class.
fn finding_key(outcome: Outcome, what: &str, seed_file: &str) -> String {
    if outcome == Outcome::Hang { format!("Hang:{seed_file}") } else { format!("{outcome:?}:{}", class(what)) }
}

/// Everything one job needs, so that the loop reads as what it does.
struct Job<'a> {
    index: usize,
    exe: &'a Path,
    seeds: &'a [(String, Vec<u8>)],
    work: &'a Path,
    findings: &'a Path,
    options: &'a Options,
    start: Instant,
    done: &'a AtomicUsize,
    /// Set when a job cannot go on, so that the others stop too.
    stop: &'a AtomicBool,
    found: &'a Mutex<Vec<(Outcome, String, String)>>,
    solo: &'a Mutex<()>,
}

impl Job<'_> {
    fn run(&self) -> Result<(), Infrastructure> {
        let (file, stderr_file) = (self.work.join(format!("job{}.xml", self.index)), self.work.join(format!("job{}.err", self.index)));
        // Each job minimises in a file of its own: a shared one would mix up what two jobs are probing.
        let probe = self.work.join(format!("probe{}.xml", self.index));
        let mut rng = Rng(self.options.seed.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(self.index as u64 + 1) | 1);
        while self.start.elapsed() < self.options.time && !self.stop.load(Ordering::Relaxed) {
            if self.done.fetch_add(1, Ordering::Relaxed) >= self.options.iterations {
                break;
            }
            let Some((name, base)) = self.seeds.get(rng.below(self.seeds.len())) else { break };
            let other = self.seeds.get(rng.below(self.seeds.len())).map(|s| s.1.as_slice()).unwrap_or_default();
            let data = mutate(&mut rng, base, other);
            std::fs::write(&file, &data).with_context(|| format!("writing {}", file.display()))?;
            let (outcome, what) = run_one(self.exe, &file, &stderr_file, self.options.timeout)?;
            if outcome == Outcome::Ok || locked(self.found).iter().any(|(o, w, s)| finding_key(*o, w, s) == finding_key(outcome, &what, name)) {
                continue;
            }
            // A hang under full load may just be a slow machine: confirm it alone, with three times the time, before recording it.
            // Hangs are not minimised (each probe costs a full timeout) and are kept as they are.
            let small = if outcome == Outcome::Hang {
                let _guard = locked(self.solo);
                if run_one(self.exe, &file, &stderr_file, self.options.timeout * 3)?.0 != Outcome::Hang {
                    continue;
                }
                data.clone()
            } else {
                minimize(self.exe, &probe, &stderr_file, data.clone(), outcome, &what, self.options.timeout)?
            };
            // Another job may have found the same while this one minimised.
            {
                let mut seen = locked(self.found);
                if seen.iter().any(|(o, w, s)| finding_key(*o, w, s) == finding_key(outcome, &what, name)) {
                    continue;
                }
                seen.push((outcome, what.clone(), name.clone()));
            }
            let id = format!("{:?}-{:016x}", outcome, fxhash(&small)).to_lowercase();
            let note = serde_json::json!({ "outcome": format!("{outcome:?}"), "detail": what, "seed_file": name, "bytes": small.len(), "original_bytes": data.len() });
            std::fs::write(self.findings.join(format!("{id}.xml")), &small).with_context(|| format!("writing the finding {id}.xml"))?;
            std::fs::write(self.findings.join(format!("{id}.json")), serde_json::to_string_pretty(&note)?)
                .with_context(|| format!("writing the finding {id}.json"))?;
            println!("  {outcome:?} from {name}: {what} → fuzz-out/findings/{id}.xml ({} bytes)", small.len());
        }
        Ok(())
    }
}

pub fn run(args: &[String]) -> Result<()> {
    let options = Options::parse(args)?;
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
    let seeds = seeds();
    println!(
        "fuzz: {} seeds, {} jobs, {} s, timeout {} s, seed {}",
        seeds.len(),
        options.jobs,
        options.time.as_secs(),
        options.timeout.as_secs(),
        options.seed
    );

    let done = AtomicUsize::new(0);
    let found: Mutex<Vec<(Outcome, String, String)>> = Mutex::default();
    // Held while confirming a hang; the other jobs keep running, so this only serialises confirmations with each other.
    let solo = Mutex::new(());
    let infrastructure: Mutex<Vec<Infrastructure>> = Mutex::default();
    let stop = AtomicBool::new(false);
    let start = Instant::now();
    std::thread::scope(|scope| {
        for index in 0..options.jobs {
            let job = Job {
                index,
                exe: &exe,
                seeds: &seeds,
                work: &work,
                findings: &findings,
                options: &options,
                start,
                done: &done,
                stop: &stop,
                found: &found,
                solo: &solo,
            };
            let (infrastructure, stop) = (&infrastructure, &stop);
            scope.spawn(move || {
                if let Err(e) = job.run() {
                    stop.store(true, Ordering::Relaxed);
                    locked(infrastructure).push(e);
                }
            });
        }
    });
    if let Some(e) = locked(&infrastructure).pop() {
        return Err(e.context("the fuzzer could not run its checks (nothing is known about the inputs it did not get to)"));
    }
    let runs = done.load(Ordering::Relaxed).min(options.iterations);
    let found = locked(&found);
    let crashes = found.iter().filter(|f| f.0 == Outcome::Crash).count();
    let hangs = found.len() - crashes;
    println!(
        "fuzz: {runs} runs in {:.0} s ({:.1}/s): {crashes} distinct crash(es), {hangs} hang(s)",
        start.elapsed().as_secs_f64(),
        runs as f64 / start.elapsed().as_secs_f64().max(0.001)
    );
    if runs == 0 {
        bail!("no input was run: the time was too short or there were no seeds");
    }
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

    fn options(line: &str) -> Result<Options> {
        Options::parse(&line.split_whitespace().map(str::to_owned).collect::<Vec<_>>())
    }

    #[test]
    fn the_options_are_all_checked() {
        let parsed = options("--time 60 --iterations 5 --seed 9 --jobs 3 --timeout 20").expect("valid");
        assert_eq!(parsed, Options { time: Duration::from_secs(60), iterations: 5, seed: 9, jobs: 3, timeout: Duration::from_secs(20) });
        let defaults = options("").expect("defaults");
        assert_eq!(
            (defaults.time, defaults.seed, defaults.timeout, defaults.iterations),
            (Duration::from_secs(300), 1, Duration::from_secs(10), usize::MAX)
        );
        assert!((1..=MAX_JOBS).contains(&defaults.jobs));
        for (line, needle) in [
            ("--time", "needs a value"),
            ("--time x", "needs a number"),
            ("--time 0", "between 1 and"),
            ("--jobs 0", "between 1 and 64"),
            ("--jobs 65", "between 1 and 64"),
            ("--timeout 0", "between 1 and"),
            ("--iterations 0", "between 1 and"),
            ("--what 3", "unknown option --what"),
            ("3", "unknown option 3"),
            ("--seed 1 --seed 2", "given twice"),
        ] {
            let err = options(line).expect_err(line).to_string();
            assert!(err.contains(needle), "{line:?}: {err}");
        }
    }

    #[test]
    fn failures_are_the_same_bug_when_their_class_is_the_same() {
        let a = "exit status: 1; xmlcraft-cli: self-check failed: step 12 (kind 4): a caret at 55 is not on a character boundary of the 300 bytes of text";
        let b =
            "exit status: 1; xmlcraft-cli: self-check failed: step 3 (kind 4): a caret at 7 is not on a character boundary of the 12 bytes of text";
        assert_eq!(class(a), class(b));
        assert_ne!(class(a), class("exit status: 1; xmlcraft-cli: self-check failed: undoing every step did not give back the original text"));
        // A panic is where it happened: two places are two bugs.
        let p1 = "exit status: 101; thread 'main' panicked at crates/x/src/y.rs:10:5:";
        let p2 = "exit status: 101; thread 'main' panicked at crates/x/src/y.rs:11:5:";
        assert_ne!(class(p1), class(p2));
        assert_eq!(finding_key(Outcome::Hang, "over 10 s", "a.xml"), "Hang:a.xml");
        assert_eq!(finding_key(Outcome::Crash, a, "a.xml"), finding_key(Outcome::Crash, b, "other.xml"));
    }

    #[test]
    fn a_child_that_cannot_start_stops_the_run_instead_of_passing_every_input() {
        let dir = tempfile::tempdir().expect("dir");
        let file = dir.path().join("a.xml");
        std::fs::write(&file, b"<a/>").expect("writes");
        let missing = dir.path().join("no-such-program");
        let err = run_one(&missing, &file, &dir.path().join("err"), Duration::from_secs(1)).expect_err("cannot start");
        assert!(format!("{err:#}").contains("starting") || format!("{err:#}").contains("could not be run"), "{err:#}");
        // A work file that cannot be written is an error too.
        let err = run_one(&missing, &file, &dir.path().join("no-such-directory/err"), Duration::from_secs(1)).expect_err("cannot write");
        assert!(format!("{err:#}").contains("creating"), "{err:#}");
    }

    #[cfg(unix)]
    #[test]
    fn exit_codes_126_and_127_are_the_shell_failing_to_run_the_program_not_a_finding() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("dir");
        let file = dir.path().join("a.xml");
        std::fs::write(&file, b"<a/>").expect("writes");
        let script = |name: &str, body: &str| {
            let path = dir.path().join(name);
            std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("writes");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
            path
        };
        let err = dir.path().join("err");
        for code in [126, 127] {
            let failing = script(&format!("exit{code}"), &format!("echo 'sh: no such program' >&2; exit {code}"));
            let message = run_one(&failing, &file, &err, Duration::from_secs(5)).expect_err("infrastructure").to_string();
            assert!(message.contains("could not be run") && message.contains("no such program"), "{message}");
        }
        // Any other failure of the child is the input's.
        let crash = script("exit1", "echo 'xmlcraft-cli: self-check failed: x' >&2; exit 1");
        let (outcome, what) = run_one(&crash, &file, &err, Duration::from_secs(5)).expect("a finding");
        assert_eq!(outcome, Outcome::Crash);
        assert!(what.contains("self-check failed"), "{what}");
        let fine = script("exit0", "exit 0");
        assert_eq!(run_one(&fine, &file, &err, Duration::from_secs(5)).expect("passes").0, Outcome::Ok);
    }

    #[test]
    fn the_reason_names_the_panic_or_the_broken_promise_without_the_thread_id() {
        let panic = "thread 'main' (4242) panicked at crates/x/src/y.rs:1:2:\nboom";
        assert_eq!(reason(panic), "thread 'main' panicked at crates/x/src/y.rs:1:2:");
        assert_eq!(reason("xmlcraft-cli: self-check failed: undoing everything\n"), "xmlcraft-cli: self-check failed: undoing everything");
        assert_eq!(reason("nothing of interest"), "");
    }
}
