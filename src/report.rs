//! Run state on disk, a run lock, and Warnflare notifications.

use crate::config::{self, Warnflare};
use crate::mirror::Report;
use std::fs::File;
use std::path::Path;

/// Write the run summary atomically (Warnflare freshness rules read it).
pub fn save(path: &Path, r: &Report) {
    let tmp = path.with_extension("json.tmp");
    if let Ok(text) = serde_json::to_string(r) {
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, path);
        }
    }
}

/// A failed run raises `crit`; a clean run after a failure raises `info`
/// (recovered). Clean runs otherwise stay quiet.
pub fn notify(w: &Warnflare, job: &str, r: &Report) {
    let marker = config::state_dir().join(format!("{job}.failing"));
    let (severity, title, body) = if !r.failed.is_empty() {
        let _ = std::fs::write(&marker, "");
        (
            "crit",
            format!("Backup {job} failed"),
            format!(
                "{} of {} failed: {}",
                r.failed.len(),
                r.total,
                r.failed.join("; ")
            ),
        )
    } else if marker.exists() {
        let _ = std::fs::remove_file(&marker);
        (
            "info",
            format!("Backup {job} recovered"),
            format!("{}/{} mirrored", r.ok, r.total),
        )
    } else {
        return;
    };
    let Ok(token) = std::fs::read_to_string(config::expand(&w.token_file)) else {
        eprintln!("warnflare: can't read {}", w.token_file);
        return;
    };
    let body = serde_json::json!({
        "source": "gravevault",
        "severity": severity,
        "key": job,
        "title": title,
        "body": body.chars().take(3000).collect::<String>(),
    })
    .to_string();
    // curl keeps the binary free of an HTTP stack. Everything, including the
    // token, goes in a curl config on stdin: command-line arguments are
    // readable by any local user through /proc/<pid>/cmdline.
    let status = curl_post(&w.url, token.trim(), &body);
    if !status.is_ok_and(|s| s.success()) {
        eprintln!("warnflare: couldn't deliver the {severity} event");
    }
}

fn curl_post(url: &str, token: &str, body: &str) -> std::io::Result<std::process::ExitStatus> {
    use std::io::Write;
    let quote = |v: &str| format!("\"{}\"", v.replace('\\', "\\\\").replace('"', "\\\""));
    let config = format!(
        "url = {}\nrequest = \"POST\"\nheader = \"content-type: application/json\"\nheader = {}\ndata-binary = {}\nsilent\nshow-error\nfail\noutput = \"/dev/null\"\n",
        quote(url),
        quote(&format!("Authorization: Bearer {token}")),
        quote(body),
    );
    let mut child = std::process::Command::new("curl")
        .args(["--config", "-"])
        .stdin(std::process::Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(config.as_bytes())?;
    child.wait()
}

/// An exclusive, non-blocking run lock (flock), released when dropped.
pub struct Lock(#[allow(dead_code)] File);

impl Lock {
    pub fn take(path: &Path) -> Option<Lock> {
        use std::os::unix::io::AsRawFd;
        let f = File::create(path).ok()?;
        // SAFETY: flock on a file descriptor we own.
        let rc = unsafe { libc_flock(f.as_raw_fd(), 2 | 4) }; // LOCK_EX | LOCK_NB
        (rc == 0).then_some(Lock(f))
    }
}

extern "C" {
    #[link_name = "flock"]
    fn libc_flock(fd: i32, op: i32) -> i32;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// curl gets everything (token included) on stdin, and the request that
    /// arrives is exactly what we meant, even with quotes and backslashes.
    #[test]
    fn curl_post_sends_headers_and_body_intact() {
        let dir = tempfile::tempdir().unwrap();
        let capture = dir.path().join("req.json");
        let server = r#"
import http.server, json, sys
class H(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        body = self.rfile.read(int(self.headers["Content-Length"])).decode()
        json.dump({"path": self.path, "auth": self.headers["Authorization"], "ctype": self.headers["Content-Type"], "body": body}, open(sys.argv[1], "w"))
        self.send_response(202); self.end_headers()
    def log_message(self, *a): pass
s = http.server.HTTPServer(("127.0.0.1", 0), H)
print(s.server_port, flush=True)
s.handle_request()
"#;
        let mut py = std::process::Command::new("python3")
            .args(["-I", "-c", server, capture.to_str().unwrap()])
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("python3");
        let mut port = String::new();
        std::io::BufRead::read_line(
            &mut std::io::BufReader::new(py.stdout.take().unwrap()),
            &mut port,
        )
        .unwrap();
        let url = format!("http://127.0.0.1:{}/v1/events", port.trim());
        let body = r#"{"title":"a \"quoted\" path C:\\x","n":1}"#;
        let st = curl_post(&url, "wf_secret_token", body).unwrap();
        py.wait().unwrap();
        assert!(st.success());
        let got: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&capture).unwrap()).unwrap();
        assert_eq!(got["path"], "/v1/events");
        assert_eq!(got["auth"], "Bearer wf_secret_token");
        assert_eq!(got["ctype"], "application/json");
        assert_eq!(got["body"], body);
    }
}
