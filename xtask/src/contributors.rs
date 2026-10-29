//! `cargo xtask contributors [--write] [--no-github]`: the credits of the About window, from the history of the repository.
//!
//! `contributors/contributors.json` holds commit statistics per GitHub username (and the AI models named in `Co-Authored-By` trailers); it is
//! generated here and compiled into the binary, nothing is read at run time (`docs/contributors.md`). A person's chosen names come from
//! `contributors/people.toml`, and the JSON says whether an entry is *verified*: every line of it was last written by a commit of that
//! person (`git blame`), so nobody is credited by a name someone else gave them.
//!
//! A commit author is a GitHub username when the e-mail is a GitHub `noreply` one; any other e-mail is asked of GitHub once (`gh api
//! repos/…/commits/<sha>`), unless `--no-github`. Whoever cannot be told is counted as unattributed, not guessed.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde_json::json;

use crate::gates::root;

/// A commit, as far as the credits care.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Commit {
    sha: String,
    email: String,
    date: String,
    subject: String,
    /// The names in `Co-Authored-By` trailers.
    co_authors: Vec<String>,
    added: u64,
    deleted: u64,
    binary_added: u64,
    binary_deleted: u64,
}

/// The GitHub username in a `noreply` address (`12345+name@users.noreply.github.com` or `name@users.noreply.github.com`).
fn login_of_noreply(email: &str) -> Option<String> {
    let local = email.strip_suffix("@users.noreply.github.com")?;
    let login = local.split_once('+').map_or(local, |(_, name)| name);
    (!login.is_empty() && login.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')).then(|| login.to_owned())
}

/// The commits of `git log --numstat` in the format [`LOG_FORMAT`] gives. Records start with a `\x1e`; fields are separated by `\x1f`, the
/// trailers by `\x1d`; the numstat lines follow the fields.
fn parse_log(text: &str) -> Vec<Commit> {
    text.split('\x1e')
        .filter(|record| !record.trim().is_empty())
        .filter_map(|record| {
            let (head, stats) = record.split_once('\n').unwrap_or((record, ""));
            let mut fields = head.split('\x1f');
            let sha = fields.next()?.trim().to_owned();
            let email = fields.next()?.trim().to_lowercase();
            let date = fields.next()?.trim().to_owned();
            let subject = fields.next()?.trim().to_owned();
            let trailers = fields.next().unwrap_or("");
            if sha.is_empty() {
                return None;
            }
            let co_authors = trailers
                .split('\x1d')
                .filter_map(|t| {
                    let name = t.split('<').next()?.trim();
                    (!name.is_empty()).then(|| name.to_owned())
                })
                .collect();
            let (mut added, mut deleted, mut binary_added, mut binary_deleted) = (0u64, 0u64, 0u64, 0u64);
            for line in stats.lines() {
                let mut parts = line.splitn(3, '\t');
                let (Some(a), Some(d), Some(_path)) = (parts.next(), parts.next(), parts.next()) else { continue };
                match (a.parse::<u64>(), d.parse::<u64>()) {
                    (Ok(a), Ok(d)) => {
                        added = added.saturating_add(a);
                        deleted = deleted.saturating_add(d);
                    }
                    // `-` for both: a binary file (one touched).
                    _ if a == "-" && d == "-" => {
                        binary_added = binary_added.saturating_add(1);
                        binary_deleted = binary_deleted.saturating_add(1);
                    }
                    _ => {}
                }
            }
            Some(Commit { sha, email, date, subject, co_authors, added, deleted, binary_added, binary_deleted })
        })
        .collect()
}

const LOG_FORMAT: &str = "--format=%x1e%H%x1f%ae%x1f%cI%x1f%s%x1f%(trailers:key=Co-Authored-By,valueonly,separator=%x1d)";

/// Whether a subject ends in the number of the pull request that was squash-merged (`… (#59)`).
fn merged_pull_request(subject: &str) -> bool {
    let Some(rest) = subject.trim_end().strip_suffix(')') else { return false };
    rest.rsplit_once("(#").is_some_and(|(_, n)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
}

/// Whether a name is that of an automation account (GitHub marks them `[bot]`).
fn is_bot(name: &str) -> bool {
    name.trim_end().ends_with("[bot]")
}

/// What the credits know of one person.
#[derive(Debug, Default, PartialEq, Eq)]
struct Person {
    prs: u64,
    commits: u64,
    lines_added: u64,
    lines_deleted: u64,
    binary_added: u64,
    binary_deleted: u64,
    first_commit: String,
    last_commit: String,
}

/// The totals per username, the totals per model, and how many commits no username could be told for. `login_of` maps a commit's
/// e-mail to a username.
fn aggregate(commits: &[Commit], login_of: &dyn Fn(&str) -> Option<String>) -> (BTreeMap<String, Person>, BTreeMap<String, u64>, u64) {
    let mut people: BTreeMap<String, Person> = BTreeMap::new();
    let mut models: BTreeMap<String, u64> = BTreeMap::new();
    let mut unattributed = 0;
    for commit in commits {
        // An automation account is neither a contributor nor a model (`dependabot[bot]` is both an author and a co-author).
        for model in commit.co_authors.iter().filter(|name| !is_bot(name)) {
            *models.entry(model.clone()).or_default() += 1;
        }
        let Some(login) = login_of(&commit.email) else {
            unattributed += 1;
            continue;
        };
        if is_bot(&login) {
            continue;
        }
        let person = people.entry(login).or_default();
        person.commits += 1;
        person.prs += u64::from(merged_pull_request(&commit.subject));
        person.lines_added = person.lines_added.saturating_add(commit.added);
        person.lines_deleted = person.lines_deleted.saturating_add(commit.deleted);
        person.binary_added = person.binary_added.saturating_add(commit.binary_added);
        person.binary_deleted = person.binary_deleted.saturating_add(commit.binary_deleted);
        if person.first_commit.is_empty() || commit.date < person.first_commit {
            person.first_commit.clone_from(&commit.date);
        }
        if commit.date > person.last_commit {
            person.last_commit.clone_from(&commit.date);
        }
    }
    (people, models, unattributed)
}

/// The usernames of the `[people.<login>]` sections of `people.toml`, with the 1-based line numbers of the lines that belong to each (its
/// header and every line up to the next header; comments and blank lines do not count: they say nothing about the person).
fn people_sections(text: &str) -> BTreeMap<String, Vec<usize>> {
    let mut sections: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    let mut current: Option<String> = None;
    for (index, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if let Some(header) = trimmed.strip_prefix('[').and_then(|t| t.strip_suffix(']')) {
            current = header.trim().strip_prefix("people.").map(|login| login.trim().trim_matches('"').to_owned());
            if let Some(login) = &current {
                sections.entry(login.clone()).or_default().push(index + 1);
            }
            continue;
        }
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some(login) = &current {
            sections.entry(login.clone()).or_default().push(index + 1);
        }
    }
    sections
}

/// The names someone chose in their `people.toml` entry.
#[derive(Debug, Default, PartialEq, Eq)]
struct Names {
    display_name: Option<String>,
    real_name: Option<String>,
    /// Use the name on the public GitHub profile as the display name.
    sync_github_name: bool,
}

/// The names of the entry `[people.<login>]` of `people.toml` (empty fields are none).
fn names_of(people: &toml::Table, login: &str) -> Names {
    let Some(entry) = people.get("people").and_then(toml::Value::as_table).and_then(|t| t.get(login)).and_then(toml::Value::as_table) else {
        return Names::default();
    };
    let text = |key: &str| entry.get(key).and_then(toml::Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned);
    Names {
        display_name: text("display_name"),
        real_name: text("real_name"),
        sync_github_name: entry.get("sync_github_name").and_then(toml::Value::as_bool) == Some(true),
    }
}

/// The name on the public GitHub profile of `login`.
fn github_profile_name(login: &str) -> Option<String> {
    let out = Command::new("gh").args(["api", &format!("users/{login}"), "--jq", ".name // empty"]).output().ok()?;
    let name = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    (out.status.success() && !name.is_empty()).then_some(name)
}

/// For each line of the file `git blame --line-porcelain` was run on, the e-mail of the author of the commit that last wrote it.
fn blame_authors(porcelain: &str) -> Vec<String> {
    porcelain.lines().filter_map(|line| line.strip_prefix("author-mail ")).map(|mail| mail.trim().trim_matches(['<', '>']).to_lowercase()).collect()
}

/// The usernames whose `people.toml` entry was written only by themselves: every line of it was last changed by a commit whose author is
/// that username.
fn verified_entries(sections: &BTreeMap<String, Vec<usize>>, authors: &[String], login_of: &dyn Fn(&str) -> Option<String>) -> BTreeSet<String> {
    sections
        .iter()
        .filter(|(login, lines)| {
            !lines.is_empty()
                && lines.iter().all(|line| {
                    let author = line.checked_sub(1).and_then(|index| authors.get(index));
                    author.and_then(|email| login_of(email)).is_some_and(|written_by| written_by.eq_ignore_ascii_case(login))
                })
        })
        .map(|(login, _)| login.clone())
        .collect()
}

fn git(args: &[&str]) -> Result<String> {
    let out = Command::new("git").args(args).current_dir(root()).output().with_context(|| format!("running git {}", args.join(" ")))?;
    if !out.status.success() {
        bail!("git {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// `owner/name` of the `origin` remote.
fn repository() -> Result<String> {
    let url = git(&["config", "--get", "remote.origin.url"])?;
    let url = url.trim().trim_end_matches(".git");
    let path = url.rsplit_once("github.com").map_or(url, |(_, rest)| rest).trim_start_matches([':', '/']);
    Ok(path.to_owned())
}

/// The GitHub username of the author of the commit `sha`, asked of GitHub.
fn github_login(repository: &str, sha: &str) -> Option<String> {
    let out = Command::new("gh").args(["api", &format!("repos/{repository}/commits/{sha}"), "--jq", ".author.login // empty"]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let login = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    (!login.is_empty()).then_some(login)
}

pub fn run(args: &[String]) -> Result<()> {
    let write = args.iter().any(|a| a == "--write");
    let offline = args.iter().any(|a| a == "--no-github");
    if let Some(unknown) = args.iter().find(|a| !matches!(a.as_str(), "--write" | "--no-github")) {
        bail!("unknown argument {unknown}; usage: contributors [--write] [--no-github]");
    }
    let repository = repository()?;
    let log = git(&["log", "--no-merges", "--numstat", LOG_FORMAT])?;
    let commits = parse_log(&log);
    // The username of each e-mail: a `noreply` address says it, any other is asked of GitHub with one of its commits.
    let mut logins: BTreeMap<String, Option<String>> = BTreeMap::new();
    for commit in &commits {
        logins
            .entry(commit.email.clone())
            .or_insert_with(|| login_of_noreply(&commit.email).or_else(|| if offline { None } else { github_login(&repository, &commit.sha) }));
    }
    let login_of = |email: &str| logins.get(&email.to_lowercase()).cloned().flatten();
    let (people, models, unattributed) = aggregate(&commits, &login_of);

    let people_path = root().join("contributors/people.toml");
    let people_text = std::fs::read_to_string(&people_path).unwrap_or_default();
    let people_table = people_text.parse::<toml::Table>().unwrap_or_default();
    let sections = people_sections(&people_text);
    let blame = git(&["blame", "--line-porcelain", "--", "contributors/people.toml"]).unwrap_or_default();
    // An e-mail that appears in a blame but in no commit of the log (a rewritten history) is no one.
    let verified = verified_entries(&sections, &blame_authors(&blame), &login_of);

    let head = git(&["rev-parse", "HEAD"])?;
    let head_date = git(&["log", "-1", "--format=%cI"])?;
    let mut contributors: Vec<(&String, &Person)> = people.iter().collect();
    contributors.sort_by(|a, b| b.1.commits.cmp(&a.1.commits).then(a.0.cmp(b.0)));
    let mut model_rows: Vec<(&String, &u64)> = models.iter().collect();
    model_rows.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    let document = json!({
        "schema": 1,
        "repo": repository,
        "branch": "main",
        "head": head.trim(),
        "head_date": head_date.trim(),
        "commits": commits.len(),
        "unattributed_commits": unattributed,
        "contributors": contributors.iter().map(|(login, p)| {
            // Names only from an entry its person wrote themselves.
            let names = if verified.contains(login.as_str()) { names_of(&people_table, login) } else { Names::default() };
            let github_name = if names.sync_github_name && !offline { github_profile_name(login) } else { None };
            json!({
            "login": login,
            "people_entry": verified.contains(login.as_str()),
            "display_name": names.display_name.clone().or(github_name.clone()),
            "real_name": names.real_name,
            "github_name": github_name,
            "prs": p.prs,
            "commits": p.commits,
            "lines_added": p.lines_added,
            "lines_deleted": p.lines_deleted,
            "binary_added": p.binary_added,
            "binary_deleted": p.binary_deleted,
            "first_commit": p.first_commit,
            "last_commit": p.last_commit,
        })}).collect::<Vec<_>>(),
        "models": model_rows.iter().map(|(name, commits)| json!({ "name": name, "commits": commits })).collect::<Vec<_>>(),
    });
    let text = serde_json::to_string_pretty(&document)? + "\n";
    println!(
        "contributors: {} commits, {} contributors ({} with a verified entry in people.toml), {} models, {} unattributed",
        commits.len(),
        people.len(),
        verified.len(),
        models.len(),
        unattributed
    );
    let target = root().join("contributors/contributors.json");
    if write {
        std::fs::write(&target, text).with_context(|| format!("writing {}", Path::new("contributors/contributors.json").display()))?;
        println!("wrote contributors/contributors.json");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit(email: &str, date: &str, subject: &str, co: &[&str], added: u64, deleted: u64) -> Commit {
        Commit {
            sha: "abc".to_owned(),
            email: email.to_owned(),
            date: date.to_owned(),
            subject: subject.to_owned(),
            co_authors: co.iter().map(|s| (*s).to_owned()).collect(),
            added,
            deleted,
            binary_added: 0,
            binary_deleted: 0,
        }
    }

    #[test]
    fn a_noreply_address_is_a_username() {
        assert_eq!(login_of_noreply("12345+octo-cat@users.noreply.github.com").as_deref(), Some("octo-cat"));
        assert_eq!(login_of_noreply("octocat@users.noreply.github.com").as_deref(), Some("octocat"));
        assert_eq!(login_of_noreply("someone@example.org"), None);
        assert_eq!(login_of_noreply("@users.noreply.github.com"), None);
        assert_eq!(login_of_noreply("bad name@users.noreply.github.com"), None);
    }

    #[test]
    fn the_log_is_read_with_its_statistics_trailers_and_binary_files() {
        let log = "\x1eaaa\x1fA@Example.org\x1f2026-10-01T10:00:00Z\x1fM1: one (#7)\x1fClaude Sonnet 5.5 <noreply@anthropic.com>\x1dOther Model <x@y>\n\n3\t1\tsrc/a.rs\n-\t-\tassets/i.png\n10\t0\tb.rs\n\
                   \x1ebbb\x1fb@example.org\x1f2026-10-02T10:00:00Z\x1fno trailers\x1f\n\n";
        let commits = parse_log(log);
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].email, "a@example.org", "e-mails are compared in lower case");
        assert_eq!(commits[0].co_authors, ["Claude Sonnet 5.5", "Other Model"]);
        assert_eq!((commits[0].added, commits[0].deleted, commits[0].binary_added), (13, 1, 1));
        assert!(commits[1].co_authors.is_empty());
        assert!(parse_log("").is_empty() && parse_log("\x1e\n").is_empty());
    }

    #[test]
    fn a_squashed_pull_request_is_a_subject_that_ends_in_its_number() {
        assert!(merged_pull_request("M3.12: something (#60)"));
        assert!(merged_pull_request("fix (#1)  "));
        assert!(!merged_pull_request("fixes #60"));
        assert!(!merged_pull_request("M3: x (#)"));
        assert!(!merged_pull_request("x (#6a)"));
        assert!(!merged_pull_request("x (#6) and more"));
    }

    #[test]
    fn the_totals_are_per_person_and_per_model_and_say_who_could_not_be_told() {
        let commits = [
            commit("a@x", "2026-10-02T00:00:00Z", "one (#1)", &["Model A"], 10, 2),
            commit("a@x", "2026-10-01T00:00:00Z", "two", &["Model A", "Model B"], 5, 1),
            commit("b@x", "2026-10-03T00:00:00Z", "three (#2)", &[], 1, 1),
            commit("nobody@x", "2026-10-04T00:00:00Z", "four", &["Model A"], 100, 100),
        ];
        let login_of = |email: &str| match email {
            "a@x" => Some("alice".to_owned()),
            "b@x" => Some("bob".to_owned()),
            _ => None,
        };
        let (people, models, unattributed) = aggregate(&commits, &login_of);
        let alice = &people["alice"];
        assert_eq!((alice.commits, alice.prs, alice.lines_added, alice.lines_deleted), (2, 1, 15, 3));
        assert_eq!((alice.first_commit.as_str(), alice.last_commit.as_str()), ("2026-10-01T00:00:00Z", "2026-10-02T00:00:00Z"));
        assert_eq!((people["bob"].commits, people["bob"].prs), (1, 1));
        assert_eq!(unattributed, 1, "the commit nobody could be told for is not credited to anyone");
        assert_eq!((models["Model A"], models["Model B"]), (3, 1), "a model is counted even when the author is not known");
        // An automation account is credited as neither.
        let bots = [commit("bot@x", "2026-10-05T00:00:00Z", "bump (#3)", &["dependabot[bot]", "Model A"], 1, 1)];
        let (people, models, unattributed) = aggregate(&bots, &|_| Some("dependabot[bot]".to_owned()));
        assert!(people.is_empty() && unattributed == 0);
        assert_eq!(models.keys().map(String::as_str).collect::<Vec<_>>(), ["Model A"]);
    }

    #[test]
    fn an_entry_of_people_toml_is_verified_when_its_writer_is_its_person() {
        let text = "# comment\n\n[people.alice]\nreal_name = \"Alice A\"\n# a note\ndisplay_name = \"Al\"\n\n[people.bob]\nreal_name = \"Bob B\"\n\n[people.carol]\n";
        let sections = people_sections(text);
        assert_eq!(sections["alice"], [3, 4, 6]);
        assert_eq!(sections["bob"], [8, 9]);
        assert_eq!(sections["carol"], [11]);
        // The author of every line of the file, in order (comments and blank lines are somebody's too).
        let author = |l: usize| match l {
            1..=2 => "owner@x",
            3..=6 => "alice@x",
            7..=9 => "alice@x", // Alice wrote Bob's entry: not verified
            _ => "carol@x",
        };
        let authors: Vec<String> = (1..=text.lines().count()).map(|l| author(l).to_owned()).collect();
        let login_of = |email: &str| email.strip_suffix("@x").map(str::to_owned);
        let verified = verified_entries(&sections, &authors, &login_of);
        assert_eq!(verified.iter().map(String::as_str).collect::<Vec<_>>(), ["alice", "carol"], "bob's entry was written by alice");
        // An author who is nobody (an e-mail that is no username) verifies nothing.
        assert!(verified_entries(&sections, &authors, &|_| None).is_empty());
    }

    #[test]
    fn the_names_someone_chose_are_read_and_empty_ones_are_none() {
        let table: toml::Table =
            "[people.alice]\nreal_name = \" Alice A \"\ndisplay_name = \"\"\nsync_github_name = true\n[people.bob]\ndisplay_name = \"Bobby\"\n"
                .parse()
                .expect("toml");
        assert_eq!(names_of(&table, "alice"), Names { display_name: None, real_name: Some("Alice A".to_owned()), sync_github_name: true });
        assert_eq!(names_of(&table, "bob"), Names { display_name: Some("Bobby".to_owned()), real_name: None, sync_github_name: false });
        assert_eq!(names_of(&table, "nobody"), Names::default());
        assert_eq!(names_of(&toml::Table::new(), "alice"), Names::default());
    }

    #[test]
    fn a_blame_gives_the_author_of_each_line() {
        let porcelain = "abc 1 1 1\nauthor A\nauthor-mail <A@Example.org>\nauthor-time 1\n\tline one\nabd 2 2 1\nauthor B\nauthor-mail <b@example.org>\n\tline two\n";
        assert_eq!(blame_authors(porcelain), ["a@example.org", "b@example.org"]);
    }
}
