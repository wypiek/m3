//! Which shell to run and where.

use std::path::{Path, PathBuf};

/// The user's shell: `$SHELL`, then the passwd entry, then `/bin/sh`.
/// `usable` decides whether a candidate path can be executed.
pub fn resolve_shell(env_shell: Option<&str>, passwd_shell: Option<&str>, usable: impl Fn(&Path) -> bool) -> PathBuf {
    [env_shell, passwd_shell]
        .into_iter()
        .flatten()
        .map(str::trim)
        .filter(|s| !s.is_empty() && s.starts_with('/'))
        .map(PathBuf::from)
        .find(|p| usable(p))
        .unwrap_or_else(|| PathBuf::from("/bin/sh"))
}

/// The login shell of `user` in an `/etc/passwd`-format text.
pub fn passwd_shell(passwd: &str, user: &str) -> Option<String> {
    passwd.lines().find_map(|l| {
        let f: Vec<&str> = l.split(':').collect();
        (f.len() >= 7 && f[0] == user).then(|| f[6].to_owned())
    })
}

fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// The shell for the current user.
pub fn user_shell() -> PathBuf {
    let env = std::env::var("SHELL").ok();
    let user = gtk::glib::user_name();
    let pw = std::fs::read_to_string("/etc/passwd").ok().and_then(|p| passwd_shell(&p, &user.to_string_lossy()));
    resolve_shell(env.as_deref(), pw.as_deref(), is_executable)
}

/// Directory from a VTE `file://` URI (OSC 7), if it still exists.
pub fn dir_from_uri(uri: &str) -> Option<PathBuf> {
    let (path, _host) = gtk::glib::filename_from_uri(uri).ok()?;
    path.is_dir().then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_resolution_order() {
        let all = |_: &Path| true;
        assert_eq!(resolve_shell(Some("/usr/bin/fish"), Some("/bin/bash"), all), PathBuf::from("/usr/bin/fish"));
        assert_eq!(resolve_shell(None, Some("/bin/bash"), all), PathBuf::from("/bin/bash"));
        assert_eq!(resolve_shell(Some(""), None, all), PathBuf::from("/bin/sh"));
        assert_eq!(resolve_shell(Some("zsh"), Some("/bin/zsh"), all), PathBuf::from("/bin/zsh"), "relative ignored");
        let only_bash = |p: &Path| p == Path::new("/bin/bash");
        assert_eq!(resolve_shell(Some("/missing/fish"), Some("/bin/bash"), only_bash), PathBuf::from("/bin/bash"));
        assert_eq!(resolve_shell(Some("/missing"), Some("/gone"), |_: &Path| false), PathBuf::from("/bin/sh"));
    }

    #[test]
    fn passwd_lookup() {
        let pw = "root:x:0:0::/root:/bin/bash\nme:x:1000:1000:Me:/home/me:/usr/bin/zsh\nbad line\n";
        assert_eq!(passwd_shell(pw, "me").as_deref(), Some("/usr/bin/zsh"));
        assert_eq!(passwd_shell(pw, "root").as_deref(), Some("/bin/bash"));
        assert_eq!(passwd_shell(pw, "nobody"), None);
    }

    #[test]
    fn uri_to_dir() {
        let tmp = std::env::temp_dir();
        let uri = gtk::glib::filename_to_uri(&tmp, None).map(|u| u.to_string()).unwrap_or_default();
        assert_eq!(dir_from_uri(&uri).as_deref(), Some(tmp.as_path()));
        assert_eq!(dir_from_uri("file:///definitely/not/here"), None);
        assert_eq!(dir_from_uri("not a uri"), None);
    }
}
