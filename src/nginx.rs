//! Optional nginx front-end.
//!
//! Mirrors `Cluster.setupNginx` from the Node agent, with one deliberate
//! difference: the upstream is a loopback TCP port instead of a unix socket so
//! the feature also works on Windows. nginx still owns the public port, serves
//! cached files straight off disk and forwards misses to the agent.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use regex::Regex;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio::process::{Child, Command};
use tokio::task::JoinHandle;
use tracing::{debug, info};

use crate::cluster::Cluster;
use crate::error::{Error, Result};

const TEMPLATE: &str = include_str!("../assets/nginx/nginx.conf.tmpl");
const MIME_TYPES: &str = include_str!("../assets/nginx/mime.types");

/// A running nginx process plus its access-log tailer.
pub struct Nginx {
    process: Child,
    log_task: JoinHandle<()>,
}

impl Nginx {
    /// Terminate nginx and stop counting log lines.
    pub async fn shutdown(&mut self) {
        self.log_task.abort();
        let _ = self.process.kill().await;
    }
}

/// Render, launch and monitor nginx.
pub async fn setup(
    cluster: &Arc<Cluster>,
    public_port: u16,
    https: bool,
    app_port: u16,
) -> Result<Nginx> {
    let root =
        std::env::current_dir().map_err(|e| Error::Other(format!("cannot read cwd: {e}")))?;
    let work_dir = cluster.config.tmp_dir().join("nginx");
    tokio::fs::create_dir_all(&work_dir).await?;
    tokio::fs::write(work_dir.join("mime.types"), MIME_TYPES).await?;

    let mut vars: HashMap<&str, String> = HashMap::new();
    vars.insert("root", root.to_string_lossy().replace('\\', "/"));
    vars.insert("port", public_port.to_string());
    vars.insert("appPort", app_port.to_string());
    vars.insert(
        "user",
        std::env::var("USERNAME").unwrap_or_else(|_| "nobody".into()),
    );
    vars.insert(
        "tmpdir",
        cluster
            .config
            .tmp_dir()
            .to_string_lossy()
            .replace('\\', "/"),
    );
    vars.insert("clusterId", cluster.config.cluster_id.clone());
    vars.insert(
        "disableSign",
        if cluster.config.disable_sign {
            "true"
        } else {
            "false"
        }
        .to_string(),
    );

    let config_path = work_dir.join("nginx.conf");
    tokio::fs::write(&config_path, render(TEMPLATE, &vars, https)).await?;
    debug!(path = %config_path.display(), "nginx configuration written");

    let log_path = root.join("access.log");
    ensure_file(&log_path).await?;

    let mut process = Command::new("nginx")
        .arg("-c")
        .arg(&config_path)
        .current_dir(&root)
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| Error::Other(format!("cannot start nginx: {e}")))?;

    tokio::time::sleep(Duration::from_secs(1)).await;
    if let Some(status) = process
        .try_wait()
        .map_err(|e| Error::Other(format!("cannot poll nginx: {e}")))?
    {
        return Err(Error::Other(format!(
            "nginx exited immediately with {status}"
        )));
    }

    let log_task = spawn_log_tail(Arc::clone(cluster), log_path);
    info!(port = public_port, app_port, "nginx started");
    Ok(Nginx { process, log_task })
}

async fn ensure_file(path: &Path) -> Result<()> {
    if !path.exists() {
        tokio::fs::write(path, b"").await?;
    }
    Ok(())
}

/// Tail the nginx access log and fold the served bytes into the counters.
fn spawn_log_tail(cluster: Arc<Cluster>, log_path: PathBuf) -> JoinHandle<()> {
    tokio::spawn(async move {
        // `$status $size` at the end of a combined log line.
        let pattern =
            Regex::new(r#"" (?P<status>[0-9]{3}) (?P<size>[0-9]+|-)"#).expect("valid regex");
        let mut offset: u64 = 0;
        let mut buffer = Vec::new();
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let Ok(metadata) = tokio::fs::metadata(&log_path).await else {
                continue;
            };
            if metadata.len() < offset {
                offset = 0; // log rotated
            }
            if metadata.len() == offset {
                continue;
            }
            let Ok(mut file) = tokio::fs::File::open(&log_path).await else {
                continue;
            };
            if file.seek(std::io::SeekFrom::Start(offset)).await.is_err() {
                continue;
            }
            buffer.clear();
            if file.read_to_end(&mut buffer).await.is_err() {
                continue;
            }
            offset += buffer.len() as u64;
            let text = String::from_utf8_lossy(&buffer);
            let mut hits = 0u64;
            let mut bytes = 0u64;
            for line in text.lines() {
                let Some(captures) = pattern.captures(line) else {
                    debug!(line, "cannot parse nginx log line");
                    continue;
                };
                hits += 1;
                bytes += captures
                    .name("size")
                    .and_then(|m| m.as_str().parse::<u64>().ok())
                    .unwrap_or(0);
            }
            if hits > 0 {
                cluster
                    .record_served(crate::storage::ServeStat { bytes, hits })
                    .await;
            }
        }
    })
}

/// Render the small EJS subset used by the nginx template.
pub fn render(template: &str, vars: &HashMap<&str, String>, ssl: bool) -> String {
    let mut output = resolve_conditionals(template, ssl);
    for (key, value) in vars {
        output = output.replace(&format!("<%= {key} %>"), value);
    }
    output
}

/// Resolve `<% if (ssl) { %>A<% } else { %>B<% } %>` blocks.
fn resolve_conditionals(template: &str, ssl: bool) -> String {
    let mut output = template.to_string();
    while let Some(start) = output.find("<% if (ssl) { %>") {
        let Some(else_marker) = output[start..].find("<% } else { %>") else {
            break;
        };
        let else_pos = start + else_marker;
        let Some(end_marker) = output[else_pos..].find("<% } %>") else {
            break;
        };
        let end_pos = else_pos + end_marker;

        let then_body = output[start + "<% if (ssl) { %>".len()..else_pos].to_string();
        let else_body = output[else_pos + "<% } else { %>".len()..end_pos].to_string();
        let chosen = if ssl { then_body } else { else_body };

        let mut replaced = String::with_capacity(output.len());
        replaced.push_str(&output[..start]);
        replaced.push_str(&chosen);
        replaced.push_str(&output[end_pos + "<% } %>".len()..]);
        output = replaced;
    }
    output
}

#[cfg(test)]
#[path = "nginx_test.rs"]
mod tests;
