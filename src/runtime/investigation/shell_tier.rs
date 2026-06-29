#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellTier {
    ReadOnly,
    FsMutation,
    Exec,
}

pub fn classify_shell_tier(cmd: &str) -> ShellTier {
    let cmd = cmd.trim_start();
    if cmd.is_empty() {
        return ShellTier::Exec;
    }

    // Any shell metacharacter → Exec; the runtime spawns directly (no interpreter),
    // so pipelines, redirects, and compound operators can't be executed and must be
    // steered to `bash -c` via Tier 3.
    if has_shell_metachar(cmd) {
        return ShellTier::Exec;
    }

    let mut tokens = cmd.split_whitespace();
    let program_raw = match tokens.next() {
        Some(p) => p,
        None => return ShellTier::Exec,
    };

    let program = strip_path_prefix(program_raw);
    let args: Vec<&str> = tokens.collect();

    match base_tier(program) {
        ShellTier::ReadOnly if program == "sed" => {
            // sed -i or -i<suffix> escalates to filesystem mutation. Match the flag
            // token itself — not a substring — so a script body like 's/x-info/y/'
            // does not falsely escalate.
            if args.iter().any(|a| {
                *a == "-i" || (a.starts_with("-i") && a.len() > 2) || a.starts_with("--in-place")
            }) {
                ShellTier::FsMutation
            } else {
                ShellTier::ReadOnly
            }
        }
        ShellTier::ReadOnly if program == "find" => {
            // File-writing and command-executing primaries escalate to arbitrary
            // execution: -exec/-execdir/-ok/-okdir run commands; -fprint*/-fls write files.
            if args.iter().any(|a| {
                matches!(
                    *a,
                    "-exec"
                        | "-execdir"
                        | "-ok"
                        | "-okdir"
                        | "-delete"
                        | "-fprint"
                        | "-fprintf"
                        | "-fprint0"
                        | "-fls"
                )
            }) {
                ShellTier::Exec
            } else {
                ShellTier::ReadOnly
            }
        }
        tier => tier,
    }
}

fn has_shell_metachar(cmd: &str) -> bool {
    cmd.chars()
        .any(|c| matches!(c, '|' | '>' | '<' | ';' | '$' | '`' | '&' | '\n'))
}

fn strip_path_prefix(s: &str) -> &str {
    match s.rfind('/') {
        Some(pos) => &s[pos + 1..],
        None => s,
    }
}

fn base_tier(program: &str) -> ShellTier {
    match program {
        "ls" | "find" | "cat" | "grep" | "wc" | "head" | "tail" | "sed" | "echo" | "pwd"
        | "whoami" | "which" | "file" | "stat" | "du" | "df" | "uname" | "date" | "env"
        | "printenv" => ShellTier::ReadOnly,
        "mkdir" | "rmdir" | "cp" | "mv" | "touch" | "ln" | "chmod" | "chown" | "cargo" => {
            ShellTier::FsMutation
        }
        _ => ShellTier::Exec,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ReadOnly programs

    #[test]
    fn ls_is_readonly() {
        assert_eq!(classify_shell_tier("ls -la"), ShellTier::ReadOnly);
    }

    #[test]
    fn find_no_flags_is_readonly() {
        assert_eq!(
            classify_shell_tier("find . -name '*.rs'"),
            ShellTier::ReadOnly
        );
    }

    #[test]
    fn cat_is_readonly() {
        assert_eq!(classify_shell_tier("cat Cargo.toml"), ShellTier::ReadOnly);
    }

    #[test]
    fn grep_is_readonly() {
        assert_eq!(classify_shell_tier("grep -r foo src/"), ShellTier::ReadOnly);
    }

    #[test]
    fn wc_is_readonly() {
        assert_eq!(
            classify_shell_tier("wc -l src/main.rs"),
            ShellTier::ReadOnly
        );
    }

    #[test]
    fn head_is_readonly() {
        assert_eq!(
            classify_shell_tier("head -n 20 src/lib.rs"),
            ShellTier::ReadOnly
        );
    }

    #[test]
    fn tail_is_readonly() {
        assert_eq!(
            classify_shell_tier("tail -f /var/log/syslog"),
            ShellTier::ReadOnly
        );
    }

    #[test]
    fn sed_no_inplace_is_readonly() {
        assert_eq!(
            classify_shell_tier("sed -n 's/foo/bar/p' file.txt"),
            ShellTier::ReadOnly
        );
    }

    // --- FsMutation programs ---

    #[test]
    fn mkdir_is_fsmutation() {
        assert_eq!(
            classify_shell_tier("mkdir -p /tmp/foo"),
            ShellTier::FsMutation
        );
    }

    #[test]
    fn rmdir_is_fsmutation() {
        assert_eq!(classify_shell_tier("rmdir /tmp/foo"), ShellTier::FsMutation);
    }

    #[test]
    fn cp_is_fsmutation() {
        assert_eq!(
            classify_shell_tier("cp -r src/ dst/"),
            ShellTier::FsMutation
        );
    }

    #[test]
    fn mv_is_fsmutation() {
        assert_eq!(
            classify_shell_tier("mv old.txt new.txt"),
            ShellTier::FsMutation
        );
    }

    // --- Unknown program → Exec ---

    #[test]
    fn unknown_program_is_exec() {
        assert_eq!(classify_shell_tier("python script.py"), ShellTier::Exec);
    }

    #[test]
    fn rm_is_exec() {
        assert_eq!(classify_shell_tier("rm -rf /tmp/old"), ShellTier::Exec);
    }

    #[test]
    fn cargo_is_fsmutation() {
        assert_eq!(classify_shell_tier("cargo build"), ShellTier::FsMutation);
    }

    #[test]
    fn cargo_with_subcommand_is_fsmutation() {
        assert_eq!(
            classify_shell_tier("cargo test my_filter"),
            ShellTier::FsMutation
        );
    }

    // --- Empty input → Exec ---

    #[test]
    fn empty_is_exec() {
        assert_eq!(classify_shell_tier(""), ShellTier::Exec);
    }

    #[test]
    fn whitespace_only_is_exec() {
        assert_eq!(classify_shell_tier("   "), ShellTier::Exec);
    }

    // --- Shell metacharacters → Exec ---

    #[test]
    fn pipe_is_exec() {
        assert_eq!(classify_shell_tier("ls | grep foo"), ShellTier::Exec);
    }

    #[test]
    fn redirect_out_is_exec() {
        assert_eq!(classify_shell_tier("echo hi > file.txt"), ShellTier::Exec);
    }

    #[test]
    fn redirect_in_is_exec() {
        assert_eq!(classify_shell_tier("cat < input.txt"), ShellTier::Exec);
    }

    #[test]
    fn double_ampersand_is_exec() {
        assert_eq!(classify_shell_tier("ls && echo done"), ShellTier::Exec);
    }

    #[test]
    fn double_pipe_is_exec() {
        assert_eq!(classify_shell_tier("ls || echo failed"), ShellTier::Exec);
    }

    #[test]
    fn semicolon_is_exec() {
        assert_eq!(classify_shell_tier("ls ; pwd"), ShellTier::Exec);
    }

    #[test]
    fn dollar_sign_is_exec() {
        assert_eq!(classify_shell_tier("echo $HOME"), ShellTier::Exec);
    }

    #[test]
    fn backtick_is_exec() {
        assert_eq!(classify_shell_tier("echo `pwd`"), ShellTier::Exec);
    }

    // --- Flag escalation ---

    #[test]
    fn sed_inplace_is_fsmutation() {
        assert_eq!(
            classify_shell_tier("sed -i 's/foo/bar/' file.txt"),
            ShellTier::FsMutation
        );
    }

    #[test]
    fn sed_inplace_with_suffix_is_fsmutation() {
        assert_eq!(
            classify_shell_tier("sed -i.bak 's/foo/bar/' file.txt"),
            ShellTier::FsMutation
        );
    }

    #[test]
    fn find_exec_flag_is_exec() {
        assert_eq!(
            classify_shell_tier("find . -name '*.tmp' -exec rm {} \\;"),
            ShellTier::Exec
        );
    }

    #[test]
    fn find_execdir_flag_is_exec() {
        assert_eq!(
            classify_shell_tier("find . -name '*.rs' -execdir cat {} \\;"),
            ShellTier::Exec
        );
    }

    #[test]
    fn find_delete_flag_is_exec() {
        assert_eq!(
            classify_shell_tier("find . -name '*.tmp' -delete"),
            ShellTier::Exec
        );
    }

    #[test]
    fn find_ok_flag_is_exec() {
        assert_eq!(
            classify_shell_tier("find . -name '*.tmp' -ok rm {} \\;"),
            ShellTier::Exec
        );
    }

    #[test]
    fn find_okdir_flag_is_exec() {
        assert_eq!(
            classify_shell_tier("find . -name '*.tmp' -okdir rm {} \\;"),
            ShellTier::Exec
        );
    }

    #[test]
    fn find_fprint_flag_is_exec() {
        assert_eq!(
            classify_shell_tier("find . -name '*.rs' -fprint out.txt"),
            ShellTier::Exec
        );
    }

    #[test]
    fn find_fls_flag_is_exec() {
        assert_eq!(
            classify_shell_tier("find . -fls listing.txt"),
            ShellTier::Exec
        );
    }

    #[test]
    fn sed_substring_dash_i_in_script_stays_readonly() {
        // A script body containing "-i" must not falsely escalate to FsMutation.
        assert_eq!(
            classify_shell_tier("sed -n 's/x-info/y/p' file.txt"),
            ShellTier::ReadOnly
        );
    }

    #[test]
    fn newline_is_exec() {
        assert_eq!(classify_shell_tier("ls\nrm -rf /tmp"), ShellTier::Exec);
    }

    #[test]
    fn sed_long_in_place_is_fsmutation() {
        assert_eq!(
            classify_shell_tier("sed --in-place 's/foo/bar/' file.txt"),
            ShellTier::FsMutation
        );
    }

    // --- Path-prefixed programs ---

    #[test]
    fn path_prefixed_ls_is_readonly() {
        assert_eq!(classify_shell_tier("/usr/bin/ls -la"), ShellTier::ReadOnly);
    }

    #[test]
    fn path_prefixed_rm_is_exec() {
        assert_eq!(classify_shell_tier("/bin/rm -rf /tmp/old"), ShellTier::Exec);
    }

    #[test]
    fn path_prefixed_mkdir_is_fsmutation() {
        assert_eq!(
            classify_shell_tier("/bin/mkdir -p /tmp/foo"),
            ShellTier::FsMutation
        );
    }

    // --- Leading whitespace tolerance ---

    #[test]
    fn leading_whitespace_stripped() {
        assert_eq!(classify_shell_tier("  ls -la"), ShellTier::ReadOnly);
    }
}
