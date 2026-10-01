//! The ids that carry an agent from one command to the next: a file at a ref
//! (`[PROJECT/]REPO[@REF]:PATH[:LINE[-LINE]]`), a comparison
//! (`REPO@BASE..HEAD`), a review thread (`PR/THREAD`), and the web URLs they
//! stand for. A URL is read, never fetched.

use agent_cli_core::{Failure, status_of};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Serialize;

use crate::client::{Ado, Kind, query_value, segment};

/// A file, or a folder, at a ref, perhaps narrowed to lines.
#[derive(Debug, PartialEq)]
pub(crate) struct FileId {
    /// `None` is `[ado] code_project`.
    pub(crate) project: Option<String>,
    pub(crate) repo: String,
    /// A branch, tag or commit as typed; `None` is the default branch.
    pub(crate) reference: Option<String>,
    /// Without a leading `/`; empty is the repository's root.
    pub(crate) path: String,
    pub(crate) lines: Option<(usize, usize)>,
}

impl FileId {
    /// `api@main:src/x.cs:40-60`, `worker:src/Jobs`, or a file's web URL
    /// (`…/_git/api?path=/src/x.cs&version=GBmain&line=42`). `ref_flag` and
    /// `lines_flag` are the same pieces as flags; one that disagrees with the
    /// id is exit 2.
    pub(crate) fn parse(
        ado: &Ado,
        raw: &str,
        ref_flag: Option<&str>,
        lines_flag: Option<&str>,
    ) -> Result<Self> {
        let raw = raw.trim();
        let bad = |why: String| -> anyhow::Error {
            Failure::usage(why)
                .hint("name a file as REPO[@REF]:PATH[:LINE[-LINE]], e.g. api@main:src/Program.cs:42, or pass its web URL")
                .into()
        };
        let mut id = match web(ado, raw).map_err(bad)? {
            Some((segments, query)) => from_url(&segments, &query)
                .ok_or_else(|| bad(format!("{raw} is not a file's URL")))?,
            None => from_text(raw).ok_or_else(|| bad(format!("{raw:?} is not a file id")))?,
        };
        id.project = id
            .project
            .filter(|project| !project.eq_ignore_ascii_case(&ado.code_project));
        if let Some(flag) = ref_flag {
            agree("--ref", &mut id.reference, flag.trim().to_owned())?;
        }
        if let Some(flag) = lines_flag {
            let lines = line_range(flag)
                .ok_or_else(|| bad(format!("--line {flag:?} is not LINE or A-B")))?;
            agree("--line", &mut id.lines, lines)?;
        }
        Ok(id)
    }

    /// `--repo` beside an id that names its own repository must name the same.
    pub(crate) fn agree_repo(&self, flag: Option<&str>) -> Result<()> {
        let Some(flag) = flag else { return Ok(()) };
        let repo = flag.rsplit('/').next().unwrap_or(flag);
        if repo.eq_ignore_ascii_case(&self.repo) {
            return Ok(());
        }
        Err(Failure::usage(format!(
            "--repo {flag:?} disagrees with the id's {:?}",
            self.repo
        ))
        .hint("drop --repo, or the repository in the id")
        .into())
    }

    /// The project its requests go to.
    pub(crate) fn project<'a>(&'a self, ado: &'a Ado) -> &'a str {
        self.project.as_deref().unwrap_or(&ado.code_project)
    }

    /// The same file at `lines`, as `file get` takes it.
    pub(crate) fn at(&self, lines: Option<(usize, usize)>) -> String {
        file_id(
            self.project.as_deref(),
            &self.repo,
            self.reference.as_deref(),
            &self.path,
            lines,
        )
    }
}

/// A bare path beside `--repo`: `src/x.cs:42` with `--repo api` is
/// `api:src/x.cs:42`. A URL, or an id whose part before the first `:` reads
/// as a repository (no `/` or `.`, or `PROJECT/` and `--repo`'s name), is
/// left as it is, for [`FileId::agree_repo`] to check.
pub(crate) fn with_repo(raw: &str, repo: Option<&str>) -> String {
    let Some(repo) = repo else {
        return raw.to_owned();
    };
    let head = raw.split(':').next().unwrap_or_default();
    let name = head.split('@').next().unwrap_or_default();
    let named = |repo: &str| name.rsplit('/').next() == repo.rsplit('/').next();
    let id = raw.starts_with("https://")
        || (raw.contains(':') && (!(name.contains('/') || name.contains('.')) || named(repo)));
    if id {
        raw.to_owned()
    } else {
        format!("{repo}:{}", raw.trim_start_matches('/'))
    }
}

/// `[PROJECT/]REPO[@REF]:PATH[:LINE[-LINE]]`; a path without a leading `/`.
pub(crate) fn file_id(
    project: Option<&str>,
    repo: &str,
    reference: Option<&str>,
    path: &str,
    lines: Option<(usize, usize)>,
) -> String {
    let mut id = project
        .map(|project| format!("{project}/"))
        .unwrap_or_default();
    id.push_str(repo);
    if let Some(reference) = reference {
        id.push('@');
        id.push_str(reference);
    }
    id.push(':');
    id.push_str(path.trim_start_matches('/'));
    match lines {
        Some((first, last)) if first == last => id.push_str(&format!(":{first}")),
        Some((first, last)) => id.push_str(&format!(":{first}-{last}")),
        None => {}
    }
    id
}

/// A piece given in the id and as a flag must say the same.
pub(crate) fn agree<T: PartialEq + std::fmt::Debug>(
    flag: &str,
    held: &mut Option<T>,
    given: T,
) -> Result<()> {
    match held {
        Some(held) if *held != given => Err(Failure::usage(format!(
            "{flag} {given:?} disagrees with the id's {held:?}"
        ))
        .hint(format!("drop {flag}, or the piece in the id"))
        .into()),
        _ => {
            *held = Some(given);
            Ok(())
        }
    }
}

fn from_text(raw: &str) -> Option<FileId> {
    // Git refs cannot hold `:`, so the first one ends the repository part.
    let (head, rest) = raw.split_once(':').unwrap_or((raw, ""));
    let (left, reference) = match head.split_once('@') {
        Some((left, reference)) => (left, Some(reference)),
        None => (head, None),
    };
    let (project, repo) = match left.split_once('/') {
        Some((project, repo)) => (Some(project), repo),
        None => (None, left),
    };
    let (path, lines) = match rest
        .rsplit_once(':')
        .and_then(|(path, lines)| Some((path, line_range(lines)?)))
    {
        Some((path, lines)) => (path, Some(lines)),
        None => (rest, None),
    };
    if repo.is_empty() || reference == Some("") || project == Some("") {
        return None;
    }
    Some(FileId {
        project: project.map(str::to_owned),
        repo: repo.to_owned(),
        reference: reference.map(str::to_owned),
        path: path.trim_matches('/').to_owned(),
        lines,
    })
}

/// `…/{project}/_git/{repo}?path=/x&version=GBmain&line=42&lineEnd=44`;
/// `GT` is a tag, `GC` a commit.
fn from_url(segments: &[String], query: &str) -> Option<FileId> {
    let at = segments
        .iter()
        .position(|segment| segment.eq_ignore_ascii_case("_git"))?;
    let reference =
        query_param(query, "version").map(|version| match (version.get(..2), version.get(2..)) {
            (Some("GT"), Some(tag)) => format!("refs/tags/{tag}"),
            (Some("GB" | "GC"), Some(name)) => name.to_owned(),
            _ => version,
        });
    let line = |name: &str| query_param(query, name)?.parse::<usize>().ok();
    let lines = line("line").map(|first| (first, line("lineEnd").unwrap_or(first).max(first)));
    Some(FileId {
        project: at.checked_sub(1).map(|before| segments[before].clone()),
        repo: segments.get(at + 1)?.clone(),
        reference,
        path: query_param(query, "path")
            .unwrap_or_default()
            .trim_matches('/')
            .to_owned(),
        lines,
    })
}

/// `42` or `40-60`, counted from 1.
pub(crate) fn line_range(raw: &str) -> Option<(usize, usize)> {
    let (first, last) = raw
        .trim()
        .split_once('-')
        .unwrap_or((raw.trim(), raw.trim()));
    let (first, last) = (first.parse().ok()?, last.parse().ok()?);
    (first >= 1 && last >= first).then_some((first, last))
}

/// Two refs of one repository: `[PROJECT/]REPO@BASE..HEAD`.
#[derive(Debug, PartialEq)]
pub(crate) struct Range {
    pub(crate) project: Option<String>,
    pub(crate) repo: String,
    pub(crate) base: String,
    pub(crate) head: String,
}

impl Range {
    pub(crate) fn parse(ado: &Ado, raw: &str) -> Result<Self> {
        let parsed = raw.trim().split_once('@').and_then(|(left, refs)| {
            let (base, head) = refs.split_once("..")?;
            let (project, repo) = match left.split_once('/') {
                Some((project, repo)) => (Some(project.to_owned()), repo),
                None => (None, left),
            };
            [repo, base, head]
                .iter()
                .all(|part| !part.is_empty() && !part.contains(':'))
                .then(|| Self {
                    project: project
                        .filter(|project| !project.eq_ignore_ascii_case(&ado.code_project)),
                    repo: repo.to_owned(),
                    base: base.to_owned(),
                    head: head.to_owned(),
                })
        });
        parsed.ok_or_else(|| {
            Failure::usage(format!(
                "{raw:?} is neither a pull request nor REPO@BASE..HEAD"
            ))
            .hint("agent-cli ado diff get api@v1.4.1..v1.4.2")
            .into()
        })
    }
}

/// A review thread: `436/7`, its web URL (`…/pullrequest/436?discussionId=7`),
/// or the thread's number with `pr` (`--pr`) naming the pull request.
pub(crate) fn thread_id(ado: &Ado, raw: &str, pr: Option<&str>) -> Result<(i64, i64)> {
    let raw = raw.trim();
    let bad = |why: String| -> anyhow::Error {
        Failure::usage(why)
            .hint("name a thread as PR/THREAD (436/7), as `agent-cli ado thread list PR` prints it")
            .into()
    };
    let (from_id, thread) = match web(ado, raw).map_err(bad)? {
        Some((_, query)) => {
            let thread = query_param(&query, "discussionId").and_then(|id| id.parse().ok());
            let pr = ado.id(Kind::PullRequest, raw).ok();
            match (pr, thread) {
                (Some(pr), Some(thread)) => (Some(pr), thread),
                _ => return Err(bad(format!("{raw} is not a thread's URL"))),
            }
        }
        None => {
            let (pr, thread) = match raw.rsplit_once('/') {
                Some((pr, thread)) => (Some(ado.id(Kind::PullRequest, pr)?), thread),
                None => (None, raw),
            };
            let thread = thread
                .parse::<i64>()
                .ok()
                .filter(|thread| *thread > 0)
                .ok_or_else(|| bad(format!("{raw:?} is not a thread id")))?;
            (pr, thread)
        }
    };
    let mut held = from_id;
    if let Some(flag) = pr {
        agree("--pr", &mut held, ado.id(Kind::PullRequest, flag)?)?;
    }
    let pr = held.ok_or_else(|| bad(format!("{raw:?} does not say which pull request")))?;
    Ok((pr, thread))
}

/// The readings of a ref as typed, as Azure DevOps `versionType`s, in the
/// order to try them: 7 to 40 hex digits is a commit, `refs/heads/` and
/// `refs/tags/` say which, and anything else is a branch, else a tag.
fn readings(reference: &str) -> Vec<(&'static str, String)> {
    let reference = reference.trim();
    if (7..=40).contains(&reference.len()) && reference.bytes().all(|b| b.is_ascii_hexdigit()) {
        return vec![("commit", reference.to_owned())];
    }
    if let Some(branch) = reference.strip_prefix("refs/heads/") {
        return vec![("branch", branch.to_owned())];
    }
    if let Some(tag) = reference.strip_prefix("refs/tags/") {
        return vec![("tag", tag.to_owned())];
    }
    vec![
        ("branch", reference.to_owned()),
        ("tag", reference.to_owned()),
    ]
}

/// `fetch` with each reading of `refs` in turn (see [`readings`]) until one
/// is not a 404. When every reading fails, the first failure stands: it is
/// about the likelier reading.
pub(crate) fn resolving<T>(
    refs: &[&str],
    mut fetch: impl FnMut(&[(&'static str, String)]) -> Result<T>,
) -> Result<T> {
    let each: Vec<Vec<(&'static str, String)>> = refs.iter().map(|r| readings(r)).collect();
    let tries = each.iter().map(Vec::len).max().unwrap_or(1);
    let (mut at, mut first) = (0, None);
    loop {
        let pick: Vec<(&'static str, String)> = each
            .iter()
            .map(|readings| readings[at.min(readings.len() - 1)].clone())
            .collect();
        match fetch(&pick) {
            Err(error) if status_of(&error) == Some(404) && at + 1 < tries => {
                first.get_or_insert(error);
                at += 1;
            }
            Err(error) if status_of(&error) == Some(404) => return Err(first.unwrap_or(error)),
            done => return done,
        }
    }
}

/// `versionDescriptor.version=…&versionDescriptor.versionType=…&`, or
/// nothing for the default branch.
pub(crate) fn version_query(reading: Option<&(&str, String)>) -> String {
    reading.map_or_else(String::new, |(kind, version)| {
        format!(
            "versionDescriptor.version={}&versionDescriptor.versionType={kind}&",
            query_value(version)
        )
    })
}

/// `git/repositories/{repo}/items?path=…`: one file or folder.
pub(crate) fn items_path(repo: &str) -> String {
    format!("git/repositories/{}/items", segment(repo))
}

/// `*` is any run of characters and `?` one. A pattern without a `/`
/// matches the file's name, one with a `/` its whole path.
pub(crate) fn glob(pattern: &str, path: &str) -> bool {
    let path = path.trim_start_matches('/');
    let pattern = pattern.trim().trim_start_matches('/');
    let subject = if pattern.contains('/') {
        path
    } else {
        path.rsplit('/').next().unwrap_or(path)
    };
    let (p, s): (Vec<char>, Vec<char>) = (pattern.chars().collect(), subject.chars().collect());
    let (mut pi, mut si, mut star, mut mark) = (0, 0, None, 0);
    while si < s.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == s[si]) {
            pi += 1;
            si += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = si;
            pi += 1;
        } else if let Some(at) = star {
            pi = at + 1;
            mark += 1;
            si = mark;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|c| *c == '*')
}

/// The decoded path segments after the organization, and the query, of an
/// Azure DevOps web URL in this organization. `Ok(None)`: `raw` is no URL.
/// `Err`: why it is not one of this organization's.
pub(crate) fn web(ado: &Ado, raw: &str) -> Result<Option<(Vec<String>, String)>, String> {
    let Some(rest) = raw.strip_prefix("https://") else {
        return Ok(None);
    };
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    let host = host.to_ascii_lowercase();
    let (org, path) = match host.strip_suffix(".visualstudio.com") {
        Some(org) => (org.to_owned(), path),
        None if host == "dev.azure.com" => {
            let (org, path) = path.split_once('/').unwrap_or((path, ""));
            (org.to_ascii_lowercase(), path)
        }
        None => return Err(format!("{raw} is not an Azure DevOps URL")),
    };
    if org != ado.org.to_ascii_lowercase() {
        return Err(format!(
            "{raw} is in organization {org}, and [ado] org is {}",
            ado.org
        ));
    }
    let (path, query) = path.split_once('?').unwrap_or((path, ""));
    let segments = path.split('/').map(decode).collect();
    Ok(Some((
        segments,
        query.split('#').next().unwrap_or("").to_owned(),
    )))
}

/// One query parameter's value, decoded.
pub(crate) fn query_param(query: &str, name: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == name).then(|| decode(&value.replace('+', " ")))
    })
}

/// Every `%XX` decoded.
fn decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        let hex = bytes
            .get(at + 1..at + 3)
            .and_then(|pair| std::str::from_utf8(pair).ok())
            .and_then(|pair| u8::from_str_radix(pair, 16).ok());
        match (bytes[at], hex) {
            (b'%', Some(byte)) => {
                out.push(byte);
                at += 3;
            }
            (byte, _) => {
                out.push(byte);
                at += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// An id or a name in a printed command line, quoted when a shell would
/// split it or a note's `]` would end it: `--task 'Run tests'`.
pub(crate) fn arg(word: &str) -> String {
    let plain = !word.is_empty()
        && word
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_./:=,@%+#".contains(&byte));
    match (plain, word.contains('\'')) {
        (true, _) => word.to_owned(),
        (false, false) => format!("'{word}'"),
        (false, true) => format!("\"{word}\""),
    }
}

/// What a verb taking `ID…` prints: the object for one id, an array in the
/// order given for several.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum Each<T> {
    One(T),
    Several(Vec<T>),
}

/// `one` over each of `ids`, so several ids cost the agent one call. One id
/// answers or fails as `one` does. With several, every id has its turn; a
/// failed one then fails the command with the others as data. An error that
/// is no `Failure` (a dry run stops at each change, after planning it) is
/// returned as it is.
pub(crate) fn each<T: Serialize>(
    ids: &[String],
    mut one: impl FnMut(&str) -> Result<T>,
) -> Result<Each<T>> {
    fn failure_of(error: &anyhow::Error) -> Option<&Failure> {
        error
            .chain()
            .find_map(|cause| cause.downcast_ref::<Failure>())
    }
    if let [id] = ids {
        return one(id).map(Each::One);
    }
    let (mut rows, mut failed) = (Vec::new(), Vec::new());
    for id in ids {
        match one(id) {
            Ok(row) => rows.push(row),
            Err(error) => failed.push((id, error)),
        }
    }
    if let Some(at) = failed
        .iter()
        .position(|(_, error)| failure_of(error).is_none())
    {
        return Err(failed.swap_remove(at).1);
    }
    let Some(first) = failed.first().and_then(|(_, error)| failure_of(error)) else {
        return Ok(Each::Several(rows));
    };
    let message = failed
        .iter()
        .map(|(id, error)| format!("{id}: {error:#}"))
        .collect::<Vec<_>>()
        .join("; ");
    let mut failure = Failure::new(first.exit, message).with_data(&rows);
    failure.hint.clone_from(&first.hint);
    failure.status = first.status;
    Err(failure.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::CONFIG;

    fn ado() -> Ado {
        let config = agent_cli_core::Config::parse("config.toml", Some(CONFIG), Vec::new());
        Ado::names(&config).unwrap()
    }

    fn file(raw: &str) -> FileId {
        FileId::parse(&ado(), raw, None, None).unwrap()
    }

    #[test]
    fn a_file_id_names_project_repo_ref_path_and_lines() {
        assert_eq!(
            file("Data/etl@refs/tags/v2:dags/a.py:40-60"),
            FileId {
                project: Some("Data".into()),
                repo: "etl".into(),
                reference: Some("refs/tags/v2".into()),
                path: "dags/a.py".into(),
                lines: Some((40, 60)),
            }
        );
        let plain = file("api:/src/x.cs");
        assert_eq!(
            (plain.project, plain.reference, plain.lines),
            (None, None, None)
        );
        assert_eq!(plain.path, "src/x.cs");
        assert_eq!(
            file("api@feature/a-b:x.cs:7").reference.as_deref(),
            Some("feature/a-b")
        );
        assert_eq!(
            file("api:odd:name.txt").path,
            "odd:name.txt",
            "a tail that is no line stays in the path"
        );
        assert_eq!(file("worker").path, "", "a repository alone is its root");
        assert_eq!(
            file("Fabrikam/api:x").project,
            None,
            "the code project is the default, so it is dropped"
        );
        assert_eq!(
            file("api@abc1234:src/x.cs:3-4").at(Some((3, 3))),
            "api@abc1234:src/x.cs:3"
        );
        for bad in ["@main:x", "api@:x", "/api:x"] {
            assert!(FileId::parse(&ado(), bad, None, None).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_file_url_reads_as_its_id() {
        let id = file(
            "https://dev.azure.com/contoso/Data%20Lake/_git/etl?path=%2Fdags%2Fa.py&version=GTv2&line=5&lineEnd=9&lineStyle=plain",
        );
        assert_eq!(id.at(id.lines), "Data Lake/etl@refs/tags/v2:dags/a.py:5-9");
        let id =
            file("https://contoso.visualstudio.com/Fabrikam/_git/api?path=/x.cs&version=GBmain");
        assert_eq!(id.at(id.lines), "api@main:x.cs");
        let elsewhere = FileId::parse(
            &ado(),
            "https://dev.azure.com/other/P/_git/api?path=/x",
            None,
            None,
        );
        let failure = elsewhere.unwrap_err();
        assert!(
            failure.to_string().contains("organization other"),
            "{failure}"
        );
    }

    #[test]
    fn a_piece_as_a_flag_fills_the_id_and_must_agree_with_it() {
        let id = FileId::parse(&ado(), "api:x.cs", Some("main"), Some("4-6")).unwrap();
        assert_eq!(id.at(id.lines), "api@main:x.cs:4-6");
        for (ref_flag, lines) in [(Some("dev"), None), (None, Some("5")), (None, Some("9-1"))] {
            let error = FileId::parse(&ado(), "api@main:x.cs:4", ref_flag, lines).unwrap_err();
            assert_eq!(error.downcast_ref::<Failure>().unwrap().exit.code(), 2);
        }
    }

    #[test]
    fn a_range_and_a_thread_id_parse_with_their_urls() {
        let range = Range::parse(&ado(), "api@v1.4.1..v1.4.2").unwrap();
        assert_eq!(
            (
                range.repo.as_str(),
                range.base.as_str(),
                range.head.as_str()
            ),
            ("api", "v1.4.1", "v1.4.2")
        );
        assert_eq!(
            Range::parse(&ado(), "Data/etl@main..dev")
                .unwrap()
                .project
                .as_deref(),
            Some("Data")
        );
        for bad in ["api", "api@v1", "api@..v2", "@a..b"] {
            assert!(Range::parse(&ado(), bad).is_err(), "{bad}");
        }
        assert_eq!(thread_id(&ado(), "436/7", None).unwrap(), (436, 7));
        assert_eq!(thread_id(&ado(), "#436/7", Some("436")).unwrap(), (436, 7));
        assert_eq!(thread_id(&ado(), "7", Some("436")).unwrap(), (436, 7));
        let url = "https://dev.azure.com/contoso/Fabrikam/_git/api/pullrequest/436?_a=files&discussionId=7";
        assert_eq!(thread_id(&ado(), url, None).unwrap(), (436, 7));
        for (raw, pr) in [("7", None), ("436/7", Some("437")), ("436/x", None)] {
            assert!(thread_id(&ado(), raw, pr).is_err(), "{raw} {pr:?}");
        }
    }

    #[test]
    fn a_bare_ref_is_tried_as_a_branch_then_a_tag_and_a_sha_as_a_commit() {
        assert_eq!(readings("abc1234"), [("commit", "abc1234".to_owned())]);
        assert_eq!(readings("refs/tags/v1"), [("tag", "v1".to_owned())]);
        assert_eq!(readings("main").len(), 2);
        let mut tried = Vec::new();
        let found = resolving(&["v1", "abc1234"], |pick| {
            tried.push(pick.to_vec());
            match pick[0].0 {
                "branch" => {
                    let mut missing = Failure::not_found("no branch");
                    missing.status = Some(404);
                    Err(missing.into())
                }
                _ => Ok(pick[1].0),
            }
        });
        assert_eq!(found.unwrap(), "commit");
        assert_eq!(tried.len(), 2);
        assert_eq!(tried[1][0], ("tag", "v1".to_owned()));
    }

    #[test]
    fn a_glob_matches_a_name_or_with_a_slash_a_path() {
        assert!(glob("*.cs", "/src/Orders/OrderClient.cs"));
        assert!(glob("Order?lient.cs", "src/Orders/OrderClient.cs"));
        assert!(glob("src/*/OrderClient.cs", "src/Orders/OrderClient.cs"));
        assert!(glob(
            "src/Orders/OrderClient.cs",
            "/src/Orders/OrderClient.cs"
        ));
        assert!(!glob("*.cs", "src/a.csproj"));
        assert!(!glob("tests/*", "src/tests/a.cs"));
    }

    #[test]
    fn a_bare_path_takes_its_repository_from_repo_and_an_id_must_agree() {
        let id =
            |raw: &str, repo| FileId::parse(&ado(), &with_repo(raw, repo), None, None).unwrap();
        assert_eq!(id("/src/x.cs:4", Some("api")), id("api:src/x.cs:4", None));
        assert_eq!(
            id("Program.cs:10", Some("api")),
            id("api:Program.cs:10", None)
        );
        assert_eq!(
            id("src/Jobs", Some("Ops/worker")).project.as_deref(),
            Some("Ops")
        );
        assert!(
            id("api:src/x.cs", Some("api"))
                .agree_repo(Some("api"))
                .is_ok()
        );
        assert!(
            id("worker:src/x.cs", Some("api"))
                .agree_repo(Some("api"))
                .is_err()
        );
    }
}
