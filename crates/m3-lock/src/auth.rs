//! PAM authentication and user lookup.
//!
//! Secrets live in `Zeroizing` buffers that are wiped when dropped. The
//! password never touches logs, never leaves this process, and is handed to
//! PAM only through the conversation callback.

use std::ffi::{CStr, CString};

use pam_client::{ConversationHandler, ErrorCode, Flag};
use zeroize::Zeroizing;

/// PAM service file: `/etc/pam.d/m3-lock`.
pub const SERVICE: &str = "m3-lock";

pub struct User {
    pub name: String,
    pub display_name: String,
    pub home: String,
}

/// Look up the *real* user from the UID (never trust `$USER` here: the
/// account authenticated must be the one owning this process).
pub fn current_user() -> Option<User> {
    // SAFETY: getpwuid_r with a sufficiently large buffer; all pointers are
    // valid for the duration of the call and the result is copied out.
    unsafe {
        let uid = libc::getuid();
        let mut pwd: libc::passwd = std::mem::zeroed();
        let mut buf = vec![0 as libc::c_char; 16384];
        let mut result: *mut libc::passwd = std::ptr::null_mut();
        let rc = libc::getpwuid_r(uid, &mut pwd, buf.as_mut_ptr(), buf.len(), &mut result);
        if rc != 0 || result.is_null() {
            return None;
        }
        let s = |p: *const libc::c_char| {
            if p.is_null() { String::new() } else { CStr::from_ptr(p).to_string_lossy().into_owned() }
        };
        let name = s(pwd.pw_name);
        let gecos = s(pwd.pw_gecos);
        let display_name =
            gecos.split(',').next().filter(|g| !g.trim().is_empty()).map(str::to_owned).unwrap_or_else(|| name.clone());
        Some(User { name, display_name, home: s(pwd.pw_dir) })
    }
}

struct Conv {
    username: String,
    password: Zeroizing<String>,
    messages: Vec<String>,
}

impl ConversationHandler for Conv {
    fn prompt_echo_on(&mut self, _msg: &CStr) -> Result<CString, ErrorCode> {
        CString::new(self.username.clone()).map_err(|_| ErrorCode::CONV_ERR)
    }

    fn prompt_echo_off(&mut self, _msg: &CStr) -> Result<CString, ErrorCode> {
        // CString copies; PAM frees (and modules usually overwrite) it.
        CString::new(self.password.as_bytes()).map_err(|_| ErrorCode::CONV_ERR)
    }

    fn text_info(&mut self, msg: &CStr) {
        self.messages.push(msg.to_string_lossy().into_owned());
    }

    fn error_msg(&mut self, msg: &CStr) {
        self.messages.push(msg.to_string_lossy().into_owned());
    }
}

pub enum AuthResult {
    Success,
    /// Wrong password, or a module message (e.g. faillock).
    Failure(String),
}

/// Authenticate `user` with `password`. Blocking: run off the UI thread.
pub fn authenticate(user: &str, password: Zeroizing<String>) -> AuthResult {
    let conv = Conv { username: user.to_owned(), password, messages: Vec::new() };
    let mut ctx = match pam_client::Context::new(SERVICE, Some(user), conv) {
        Ok(c) => c,
        Err(e) => {
            return AuthResult::Failure(format!(
                "Authentication is unavailable ({e}). Is /etc/pam.d/{SERVICE} installed?"
            ));
        }
    };
    let result = ctx.authenticate(Flag::NONE);
    // Drop the secret as soon as PAM is done with it.
    let messages = std::mem::take(&mut ctx.conversation_mut().messages);
    ctx.conversation_mut().password = Zeroizing::new(String::new());
    match result {
        Ok(()) => AuthResult::Success,
        Err(e) => {
            let msg = messages.into_iter().rev().find(|m| !m.trim().is_empty());
            AuthResult::Failure(msg.unwrap_or_else(|| match e.code() {
                ErrorCode::MAXTRIES => "Too many attempts. Try again later.".into(),
                ErrorCode::USER_UNKNOWN => "Unknown user.".into(),
                _ => "Incorrect password. Try again.".into(),
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn current_user_resolves() {
        let u = super::current_user().expect("passwd entry for the test user");
        assert!(!u.name.is_empty());
        assert!(u.home.starts_with('/'));
    }

    #[test]
    fn wrong_password_fails() {
        // Without the m3-lock PAM service file this still must fail (never
        // succeed) — authentication fails closed.
        let u = super::current_user().unwrap();
        let r = super::authenticate(&u.name, zeroize::Zeroizing::new("definitely-not-the-password".into()));
        assert!(matches!(r, super::AuthResult::Failure(_)));
    }
}
