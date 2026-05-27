use std::collections::HashSet;
use std::path::PathBuf;
use std::process::{Command, ExitStatus};

use crate::core::config::LspConfig;
use crate::core::error::{AppError, Result};

use super::types::{LspCommandSpec, LspProbe, LspProbeStatus};

pub(super) fn resolve_rust_analyzer_command(lsp_cfg: &LspConfig) -> Result<LspCommandSpec> {
    let probes = probe_rust_analyzer(lsp_cfg);
    for probe in &probes {
        if matches!(probe.status, LspProbeStatus::Ready(_)) {
            return Ok(probe.spec.clone());
        }
    }
    Err(AppError::Config(format_lsp_probe_failure(&probes)))
}

pub fn rust_lsp_health_report(lsp_cfg: &LspConfig) -> String {
    let probes = probe_rust_analyzer(lsp_cfg);
    let mut output = String::from("Rust LSP check\n\n");
    let mut found_ready = false;
    for probe in &probes {
        match &probe.status {
            LspProbeStatus::Ready(version) => {
                found_ready = true;
                output.push_str(&format!("ready: {} ({version})\n", probe.spec.display));
            }
            LspProbeStatus::Failed(reason) => {
                output.push_str(&format!("failed: {} ({reason})\n", probe.spec.display));
            }
        }
    }
    if !found_ready {
        output.push_str("\nFix:\n");
        output.push_str(
            "- Install the rust-analyzer component with `rustup component add rust-analyzer`\n",
        );
        output.push_str("- Or set [lsp].rust_analyzer_path in config.toml to a runnable binary\n");
    }
    output
}

fn probe_rust_analyzer(lsp_cfg: &LspConfig) -> Vec<LspProbe> {
    let mut probes = Vec::new();

    if let Some(path) = lsp_cfg.rust_analyzer_path.clone() {
        probes.push(run_probe(LspCommandSpec {
            display: format!("configured path {}", path.display()),
            program: path,
            args: Vec::new(),
        }));
        return probes;
    }

    for candidate in discover_rust_analyzer_candidates() {
        probes.push(run_probe(LspCommandSpec {
            display: candidate.display().to_string(),
            program: candidate,
            args: Vec::new(),
        }));
    }

    probes.push(run_probe(LspCommandSpec {
        display: "rustup run stable rust-analyzer".to_string(),
        program: PathBuf::from("rustup"),
        args: vec![
            "run".to_string(),
            "stable".to_string(),
            "rust-analyzer".to_string(),
        ],
    }));

    probes
}

fn format_lsp_probe_failure(probes: &[LspProbe]) -> String {
    let mut message = String::from(
        "rust-analyzer is not runnable. \
         Install it or set [lsp].rust_analyzer_path in config.toml.\n\nTried:\n",
    );
    for probe in probes {
        if let LspProbeStatus::Failed(reason) = &probe.status {
            message.push_str(&format!("- {}: {}\n", probe.spec.display, reason));
        }
    }
    if !rust_analyzer_component_installed() {
        message.push_str(
            "\nThe rust-analyzer rustup component is not installed for the active toolchain.\n\
             Run: rustup component add rust-analyzer\n",
        );
    }
    message
}

fn discover_rust_analyzer_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    let mut seen = HashSet::new();

    if let Some(path_var) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path_var) {
            push_candidate(&mut candidates, &mut seen, dir.join("rust-analyzer"));
        }
    }

    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        push_candidate(
            &mut candidates,
            &mut seen,
            home.join(".cargo/bin/rust-analyzer"),
        );
        push_candidate(
            &mut candidates,
            &mut seen,
            home.join(".local/bin/rust-analyzer"),
        );
    }

    push_candidate(
        &mut candidates,
        &mut seen,
        PathBuf::from("/opt/homebrew/bin/rust-analyzer"),
    );
    push_candidate(
        &mut candidates,
        &mut seen,
        PathBuf::from("/usr/local/bin/rust-analyzer"),
    );

    candidates
}

fn push_candidate(candidates: &mut Vec<PathBuf>, seen: &mut HashSet<PathBuf>, candidate: PathBuf) {
    if candidate.exists() && seen.insert(candidate.clone()) {
        candidates.push(candidate);
    }
}

fn run_probe(spec: LspCommandSpec) -> LspProbe {
    let output = Command::new(&spec.program)
        .args(&spec.args)
        .arg("--version")
        .output();

    let status = match output {
        Ok(output) => parse_probe_output(output.status, &output.stdout, &output.stderr),
        Err(e) => LspProbeStatus::Failed(e.to_string()),
    };

    LspProbe { spec, status }
}

fn parse_probe_output(status: ExitStatus, stdout: &[u8], stderr: &[u8]) -> LspProbeStatus {
    if status.success() {
        let version = String::from_utf8_lossy(stdout).trim().to_string();
        let version = if version.is_empty() {
            "version unknown".to_string()
        } else {
            version
        };
        return LspProbeStatus::Ready(version);
    }

    let stderr_str = String::from_utf8_lossy(stderr).trim().to_string();
    let stdout_str = String::from_utf8_lossy(stdout).trim().to_string();
    let detail = if !stderr_str.is_empty() {
        stderr_str
    } else if !stdout_str.is_empty() {
        stdout_str
    } else {
        format!("exit status {}", status.code().unwrap_or(-1))
    };

    LspProbeStatus::Failed(detail)
}

fn rust_analyzer_component_installed() -> bool {
    let output = Command::new("rustup")
        .args(["component", "list", "--installed"])
        .output();

    match output {
        Ok(output) if output.status.success() => String::from_utf8_lossy(&output.stdout)
            .lines()
            .any(|line| line.starts_with("rust-analyzer")),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_failure_includes_stderr() {
        let status = parse_probe_output(
            std::process::Command::new("false")
                .status()
                .expect("status"),
            b"",
            b"missing component",
        );

        match status {
            LspProbeStatus::Failed(reason) => assert!(reason.contains("missing component")),
            LspProbeStatus::Ready(_) => panic!("expected failure"),
        }
    }
}
