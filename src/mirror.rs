//! Append-only git mirror: every GitHub repo of an owner → a GitLab group.
//!
//! Never `--force`, never `--prune`: a branch deleted or rewritten on GitHub
//! can't erase the backup. A rejected (non-fast-forward) ref keeps its old
//! history on GitLab, and GitHub's new tip is saved beside it as
//! `rewritten/<branch>/<UTC stamp>`. Auth comes from the `gh` and `glab`
//! credential helpers already configured on the machine.

use crate::config::{self, GitMirror};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Default, Serialize)]
pub struct Report {
    pub at: String,
    pub total: usize,
    pub ok: usize,
    pub failed: Vec<String>,
    pub created: Vec<String>,
    pub rewritten: Vec<String>,
    pub lfs: Vec<String>,
}

/// GitLab paths can't start with a dot; GitHub's org profile repo is `.github`.
pub fn gitlab_path(github_name: &str) -> String {
    if github_name == ".github" {
        "github-org-config".into()
    } else {
        github_name.into()
    }
}

/// Refs that `git push --porcelain` rejected (lines starting with `!`).
pub fn rejected_refs(porcelain: &str) -> Vec<String> {
    porcelain
        .lines()
        .filter_map(|l| {
            let mut cols = l.split('\t');
            (cols.next()? == "!")
                .then(|| cols.next())?
                .and_then(|spec| spec.split(':').next())
                .map(str::to_string)
        })
        .collect()
}

/// Where a rejected ref's new tip is preserved.
pub fn rewritten_ref(rejected: &str, stamp: &str) -> String {
    let (kind, short) = match rejected.strip_prefix("refs/tags/") {
        Some(t) => ("tags", t),
        None => (
            "heads",
            rejected.strip_prefix("refs/heads/").unwrap_or(rejected),
        ),
    };
    format!("refs/{kind}/rewritten/{short}/{stamp}")
}

pub fn run(cfg: &GitMirror, stamp: &str, log: &mut dyn FnMut(&str)) -> Report {
    let mut report = Report {
        at: utc_now(),
        ..Default::default()
    };
    let group_id = match retry(3, log, || gitlab_group_id(&cfg.gitlab_group)) {
        Ok(id) => id,
        Err(e) => {
            log(&format!(
                "ERROR: can't reach GitLab group {}: {e}",
                cfg.gitlab_group
            ));
            report.failed.push(format!("gitlab group: {e}"));
            return report;
        }
    };
    let existing = gitlab_projects(group_id).unwrap_or_default();
    let repos = match github_repos(&cfg.github_owner) {
        Ok(r) if !r.is_empty() => r,
        Ok(_) | Err(_) => {
            report
                .failed
                .push(format!("no repositories listed for {}", cfg.github_owner));
            return report;
        }
    };
    report.total = repos.len();
    let cache = config::cache_dir().join("git");
    let _ = std::fs::create_dir_all(&cache);

    for name in &repos {
        let mut path = gitlab_path(name);
        match existing.iter().find(|p| p.eq_ignore_ascii_case(&path)) {
            Some(p) => path = p.clone(),
            None => match create_gitlab_project(group_id, name, &path) {
                Ok(()) => {
                    report.created.push(name.clone());
                    log(&format!("created gitlab project {path}"));
                }
                Err(e) => {
                    report.failed.push(format!("{name} (create: {e})"));
                    continue;
                }
            },
        }
        match mirror_one(
            cfg,
            name,
            &path,
            &cache.join(format!("{name}.git")),
            stamp,
            log,
        ) {
            Ok(m) => {
                report.ok += 1;
                report
                    .rewritten
                    .extend(m.rewritten.into_iter().map(|r| format!("{name}:{r}")));
                if m.lfs {
                    report.lfs.push(name.clone());
                }
            }
            Err(e) => {
                log(&format!("ERROR: {name}: {e}"));
                report.failed.push(format!("{name} ({e})"));
            }
        }
    }
    report
}

#[derive(Default)]
struct Mirrored {
    rewritten: Vec<String>,
    lfs: bool,
}

fn mirror_one(
    cfg: &GitMirror,
    name: &str,
    path: &str,
    bare: &Path,
    stamp: &str,
    log: &mut dyn FnMut(&str),
) -> Result<Mirrored, String> {
    let src = format!("https://github.com/{}/{name}.git", cfg.github_owner);
    let dst = format!("https://gitlab.com/{}/{path}.git", cfg.gitlab_group);
    if !bare.exists() {
        git(None, &["init", "-q", "--bare", &bare.to_string_lossy()])?;
    }
    let fetch = git(
        Some(bare),
        &[
            "fetch",
            "-q",
            "--prune",
            &src,
            "+refs/heads/*:refs/heads/*",
            "+refs/tags/*:refs/tags/*",
        ],
    );
    if let Err(e) = fetch {
        if e.contains("empty repository") || e.contains("couldn't find remote ref") {
            return Ok(Mirrored::default());
        }
        return Err(format!("fetch: {}", first_line(&e)));
    }
    if git(Some(bare), &["for-each-ref", "--count=1"])?
        .trim()
        .is_empty()
    {
        return Ok(Mirrored::default());
    }
    let out = git_any(
        Some(bare),
        &[
            "push",
            "--porcelain",
            &dst,
            "refs/heads/*:refs/heads/*",
            "refs/tags/*:refs/tags/*",
        ],
    );
    let mut m = Mirrored::default();
    for r in rejected_refs(&out) {
        let keep = rewritten_ref(&r, stamp);
        git(Some(bare), &["push", "-q", &dst, &format!("{r}:{keep}")])
            .map_err(|e| format!("keeping rewritten {r}: {}", first_line(&e)))?;
        log(&format!(
            "WARN: {name} {r} was rewritten on GitHub; old history kept, new tip saved as {keep}"
        ));
        m.rewritten.push(r);
    }
    if !out.lines().any(|l| l == "Done") && m.rewritten.is_empty() {
        return Err(format!(
            "push: {}",
            out.lines()
                .find(|l| !l.starts_with("To "))
                .unwrap_or("failed")
        ));
    }
    if let Some(src_dir) = cfg.lfs_sources.get(name) {
        git(
            Some(&config::expand(src_dir)),
            &["lfs", "push", "--all", &dst],
        )
        .map_err(|e| format!("lfs push: {}", first_line(&e)))?;
        m.lfs = true;
    } else if git(Some(bare), &["show", "HEAD:.gitattributes"])
        .is_ok_and(|a| a.contains("filter=lfs"))
    {
        log(&format!("WARN: {name} uses Git LFS but has no lfs_sources entry; its LFS files aren't backed up"));
    }
    Ok(m)
}

// ── GitHub / GitLab through their CLIs ────────────────────────────────────

fn github_repos(owner: &str) -> Result<Vec<String>, String> {
    let out = cmd(
        "gh",
        &[
            "repo", "list", owner, "--limit", "1000", "--json", "name", "--jq", ".[].name",
        ],
    )?;
    let mut v: Vec<String> = out
        .lines()
        .map(str::to_string)
        .filter(|l| !l.is_empty())
        .collect();
    v.sort();
    Ok(v)
}

fn gitlab_group_id(group: &str) -> Result<u64, String> {
    let out = cmd("glab", &["api", &format!("groups/{group}")])?;
    let v: serde_json::Value = serde_json::from_str(&out).map_err(|e| e.to_string())?;
    v["id"]
        .as_u64()
        .ok_or_else(|| "no id in group response".into())
}

fn gitlab_projects(group_id: u64) -> Result<Vec<String>, String> {
    let out = cmd(
        "glab",
        &[
            "api",
            "--paginate",
            &format!("groups/{group_id}/projects?per_page=100&include_subgroups=false"),
        ],
    )?;
    // --paginate concatenates JSON arrays; split them apart.
    let mut paths = Vec::new();
    for chunk in out.replace("][", "]\n[").lines() {
        if let Ok(serde_json::Value::Array(a)) = serde_json::from_str(chunk) {
            paths.extend(
                a.iter()
                    .filter_map(|p| p["path"].as_str().map(str::to_string)),
            );
        }
    }
    Ok(paths)
}

fn create_gitlab_project(group_id: u64, name: &str, path: &str) -> Result<(), String> {
    cmd(
        "glab",
        &[
            "api",
            "--method",
            "POST",
            "projects",
            "-f",
            &format!("name={name}"),
            "-f",
            &format!("path={path}"),
            "-f",
            &format!("namespace_id={group_id}"),
            "-f",
            "visibility=private",
        ],
    )
    .map(|_| ())
}

// ── helpers ───────────────────────────────────────────────────────────────

fn retry<T>(
    times: u32,
    log: &mut dyn FnMut(&str),
    mut f: impl FnMut() -> Result<T, String>,
) -> Result<T, String> {
    let mut last = String::new();
    for attempt in 1..=times {
        match f() {
            Ok(v) => return Ok(v),
            Err(e) => {
                log(&format!("attempt {attempt} failed: {}", first_line(&e)));
                last = e;
                if attempt < times {
                    std::thread::sleep(std::time::Duration::from_secs(30 * attempt as u64));
                }
            }
        }
    }
    Err(last)
}

fn cmd(bin: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new(bin)
        .args(args)
        .output()
        .map_err(|e| format!("{bin}: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

fn git(dir: Option<&Path>, args: &[&str]) -> Result<String, String> {
    let mut full: Vec<String> = Vec::new();
    if let Some(d) = dir {
        full.extend(["-C".into(), d.to_string_lossy().into_owned()]);
    }
    full.extend(args.iter().map(|s| s.to_string()));
    let refs: Vec<&str> = full.iter().map(String::as_str).collect();
    cmd("git", &refs)
}

/// git, returning combined output whatever the exit status (push reports
/// rejections on stdout and exits non-zero).
fn git_any(dir: Option<&Path>, args: &[&str]) -> String {
    let mut c = Command::new("git");
    if let Some(d) = dir {
        c.arg("-C").arg(d);
    }
    match c.args(args).output() {
        Ok(o) => format!(
            "{}{}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        ),
        Err(e) => e.to_string(),
    }
}

fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or(s)
}

pub fn utc_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (days, rem) = (secs / 86_400, secs % 86_400);
    // civil from days (Howard Hinnant)
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

pub fn stamp() -> String {
    utc_now().replace(['-', ':'], "")
}

#[allow(dead_code)]
pub fn bare_path(name: &str) -> PathBuf {
    config::cache_dir().join("git").join(format!("{name}.git"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_dot_github() {
        assert_eq!(gitlab_path(".github"), "github-org-config");
        assert_eq!(gitlab_path("keysmith"), "keysmith");
    }

    #[test]
    fn parses_rejections_from_porcelain() {
        let out = "To /x/dst.git\n=\trefs/heads/dev:refs/heads/dev\t[up to date]\n!\trefs/heads/main:refs/heads/main\t[rejected] (non-fast-forward)\n!\trefs/tags/v1:refs/tags/v1\t[rejected] (already exists)\nDone\n";
        assert_eq!(rejected_refs(out), ["refs/heads/main", "refs/tags/v1"]);
        assert!(rejected_refs("To x\n*\trefs/heads/a:refs/heads/a\t[new branch]\nDone").is_empty());
    }

    #[test]
    fn rewritten_refs_keep_kind_and_name() {
        assert_eq!(
            rewritten_ref("refs/heads/main", "20261021T120000Z"),
            "refs/heads/rewritten/main/20261021T120000Z"
        );
        assert_eq!(
            rewritten_ref("refs/tags/v0.4.0", "S"),
            "refs/tags/rewritten/v0.4.0/S"
        );
    }

    #[test]
    fn utc_now_is_rfc3339() {
        let s = utc_now();
        assert_eq!(s.len(), 20);
        assert!(s.ends_with('Z') && s.as_bytes()[10] == b'T');
        assert!(!stamp().contains(':'));
    }

    /// The append-only guarantee, end to end with real git (no network):
    /// a force-pushed branch keeps its old history in the mirror and the new
    /// tip lands under rewritten/.
    #[test]
    fn force_push_upstream_never_erases_the_mirror() {
        let t = tempfile::tempdir().unwrap();
        let p = |s: &str| t.path().join(s).to_string_lossy().into_owned();
        let g = |dir: &str, args: &[&str]| git(Some(Path::new(&p(dir))), args).unwrap();
        git(None, &["init", "-q", &p("work")]).unwrap();
        for m in ["one", "two"] {
            git(
                Some(Path::new(&p("work"))),
                &[
                    "-c",
                    "user.name=t",
                    "-c",
                    "user.email=t@t",
                    "commit",
                    "-q",
                    "--allow-empty",
                    "-m",
                    m,
                ],
            )
            .unwrap();
        }
        git(None, &["init", "-q", "--bare", &p("dst.git")]).unwrap();
        g(
            "work",
            &["push", "-q", &p("dst.git"), "HEAD:refs/heads/main"],
        );
        g("work", &["reset", "-q", "--hard", "HEAD~1"]);
        git(
            Some(Path::new(&p("work"))),
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "rewritten",
            ],
        )
        .unwrap();
        // Same push the mirror does: no force.
        let out = git_any(
            Some(Path::new(&p("work"))),
            &["push", "--porcelain", &p("dst.git"), "HEAD:refs/heads/main"],
        );
        let rejected = rejected_refs(&out.replace("HEAD:", "refs/heads/main:"));
        assert_eq!(rejected, ["refs/heads/main"]);
        let keep = rewritten_ref("refs/heads/main", "S");
        g(
            "work",
            &["push", "-q", &p("dst.git"), &format!("HEAD:{keep}")],
        );
        assert_eq!(
            g("dst.git", &["log", "--format=%s", "main"]).trim(),
            "two\none"
        );
        assert_eq!(
            g("dst.git", &["log", "--format=%s", &keep]).trim(),
            "rewritten\none"
        );
    }
}
