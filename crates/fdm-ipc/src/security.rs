//! Who is allowed to talk to the download list.
//!
//! The pipe is not a public interface. A client can start a download to any folder
//! this user can write to, and the `Add` request carries the `Cookie` header the
//! browser had — so "anyone who can open the pipe" needs to mean "this user's own
//! processes" and nothing else.
//!
//! Windows' default pipe security is closer to that than it first appears: the
//! default DACL grants the creator full control, `SYSTEM` and `Administrators`
//! full control, and *Everyone* read — read but not write, so an unrelated user
//! could not actually send a command even with the default. The reason for a DACL
//! anyway is that "could not send a command" is a detail of the default ACL rather
//! than a property of this program, and it costs one SDDL string to make it the
//! latter.
//!
//! # Why the SID has to be fetched rather than written into the SDDL
//!
//! There is no SDDL alias for "the user running this process". `CO` (CREATOR
//! OWNER, `S-1-3-0`) looks like one and is not: it is a placeholder that only means
//! anything in an *inheritable* ACE on a container, where it is substituted for the
//! creator as the ACE is inherited. In a DACL applied directly to an object it
//! matches no token, so a pipe protected with `(A;;FA;;;CO)` would be openable by
//! nobody at all — including FDM. Hence [`current_user_sid`].

use std::io;

use windows_sys::core::{BOOL, PWSTR};
use windows_sys::Win32::Foundation::{CloseHandle, LocalFree, HANDLE};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, TokenUser, PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES, TOKEN_QUERY,
    TOKEN_USER,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// The SID of the user this process is running as, in `S-1-5-21-…` form.
///
/// Used twice: in the pipe's DACL, and in the pipe's *name*. The name matters for
/// fast user switching — two people logged into one machine each run their own
/// FDM with their own `downloads.json`, so "FDM is already running" has to be a
/// per-user question. A shared name would make the second user's app fail to
/// start and, if it did start, would put one user's downloads in the other's list.
pub fn current_user_sid() -> io::Result<String> {
    let mut token: HANDLE = std::ptr::null_mut();
    // SAFETY: `GetCurrentProcess` returns a pseudo-handle that needs no closing,
    // and `token` is a valid out-pointer for the duration of the call.
    let opened = unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) };
    if opened == 0 {
        return Err(io::Error::last_os_error());
    }

    let sid = user_sid_from_token(token);

    // SAFETY: `token` came from a successful `OpenProcessToken` and is not used
    // again after this point.
    unsafe { CloseHandle(token) };
    sid
}

fn user_sid_from_token(token: HANDLE) -> io::Result<String> {
    let mut needed: u32 = 0;
    // The size query. It "fails" with ERROR_INSUFFICIENT_BUFFER every time — the
    // value written to `needed` is the entire point of the call, so the return
    // value is deliberately ignored and `needed` is what gets checked.
    //
    // SAFETY: a null buffer with a zero length is exactly what this call is
    // documented to take when asking for the required size.
    unsafe { GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut needed) };
    if needed == 0 {
        return Err(io::Error::last_os_error());
    }

    // A `Vec<u64>` and not a `Vec<u8>`: `TOKEN_USER` contains a pointer, and
    // reading a pointer out of a buffer that happens to be oddly aligned is
    // undefined behaviour rather than merely slow. Rounding the size up to whole
    // `u64`s makes the allocation 8-byte aligned by construction.
    let mut buf = vec![0u64; needed as usize / 8 + 1];
    let mut written = needed;
    // SAFETY: the buffer is at least `needed` bytes and correctly aligned for
    // `TOKEN_USER`; `written` is a valid out-pointer.
    let ok = unsafe {
        GetTokenInformation(
            token,
            TokenUser,
            buf.as_mut_ptr().cast(),
            needed,
            &mut written,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }

    // SAFETY: on success the buffer holds a `TOKEN_USER` whose `Sid` points into
    // the same buffer, which outlives this borrow.
    let user = unsafe { &*buf.as_ptr().cast::<TOKEN_USER>() };
    sid_to_string(user.User.Sid)
}

fn sid_to_string(sid: PSID) -> io::Result<String> {
    let mut raw: PWSTR = std::ptr::null_mut();
    // SAFETY: `sid` is a valid SID from a token, and `raw` is a valid
    // out-pointer. On success the callee allocates with `LocalAlloc`, which is
    // why this frees with `LocalFree` and not with Rust's allocator.
    if unsafe { ConvertSidToStringSidW(sid, &mut raw) } == 0 {
        return Err(io::Error::last_os_error());
    }

    // SAFETY: on success `raw` is a NUL-terminated wide string.
    let text = unsafe { wide_to_string(raw) };
    // SAFETY: `raw` came from `ConvertSidToStringSidW` and is not used again.
    unsafe { LocalFree(raw.cast()) };
    Ok(text)
}

/// # Safety
///
/// `ptr` must be non-null and point at a NUL-terminated wide string.
unsafe fn wide_to_string(ptr: PWSTR) -> String {
    let mut len = 0;
    while *ptr.add(len) != 0 {
        len += 1;
    }
    String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len))
}

/// A security descriptor built from SDDL, owned so it can be freed.
///
/// `CreateNamedPipe` copies what it needs out of the descriptor, so this only has
/// to outlive the call that creates each pipe instance — but the server creates a
/// new instance for every connection, so in practice it lives as long as the
/// server does.
pub struct SecurityDescriptor(PSECURITY_DESCRIPTOR);

// SAFETY: the pointer is owned exclusively by this value — nothing else holds a
// copy, and `attributes` hands out a `SECURITY_ATTRIBUTES` that borrows it for the
// duration of one call. The only operations on it are a read inside
// `CreateNamedPipe` and the `LocalFree` in `Drop`, neither of which cares which
// thread it happens on. Send and Sync are both needed because the accept loop is
// an ordinary spawned task, and a raw pointer would otherwise make that future
// non-`Send`.
unsafe impl Send for SecurityDescriptor {}
unsafe impl Sync for SecurityDescriptor {}

impl SecurityDescriptor {
    /// Full access for this user and for `SYSTEM`; nothing for anyone else.
    ///
    /// `D:P` protects the DACL from inheritance, and the absence of any other ACE
    /// is what does the work — a DACL with no matching ACE denies by default, so
    /// there is no need for an explicit deny entry.
    ///
    /// `SYSTEM` is included because leaving it out buys nothing: anyone who can run
    /// as `SYSTEM` on this machine can already take ownership of the pipe, so an
    /// ACL that excluded it would be a locked door in a wall that isn't there.
    /// `Administrators` is *not* included — administrators can equally well take
    /// ownership, and not listing them keeps "who talks to the download list"
    /// down to one account plus the OS.
    pub fn owner_only(user_sid: &str) -> io::Result<Self> {
        // `FA` is FILE_ALL_ACCESS, which is what GENERIC_ALL maps to for a pipe.
        Self::from_sddl(&format!("D:P(A;;FA;;;{user_sid})(A;;FA;;;SY)"))
    }

    fn from_sddl(sddl: &str) -> io::Result<Self> {
        let wide: Vec<u16> = sddl.encode_utf16().chain(std::iter::once(0)).collect();
        let mut psd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();

        // SAFETY: `wide` is NUL-terminated and outlives the call; `psd` is a valid
        // out-pointer. On success the descriptor is `LocalAlloc`-ed and becomes
        // this value's to free.
        let ok: BOOL = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                wide.as_ptr(),
                SDDL_REVISION_1,
                &mut psd,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(psd))
    }

    /// The `SECURITY_ATTRIBUTES` to hand to `CreateNamedPipe`.
    ///
    /// `bInheritHandle` is 0 deliberately. An inheritable pipe handle would be
    /// duplicated into any child process FDM spawns, and a stray copy in an
    /// unrelated child would keep the pipe alive after FDM itself had exited —
    /// which would make a restarted FDM see "already running".
    pub fn attributes(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.0,
            bInheritHandle: 0,
        }
    }
}

impl Drop for SecurityDescriptor {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: allocated by `ConvertStringSecurityDescriptorToSecurityDescriptorW`,
            // which documents `LocalFree` as the way to release it, and not
            // reachable from anywhere else.
            unsafe { LocalFree(self.0.cast()) };
        }
    }
}

/// Read a kernel object's DACL back out as SDDL.
///
/// Test-only, and the reason it exists is that everything else about the ACL is
/// unfalsifiable from inside the process that set it: building a
/// `SECURITY_ATTRIBUTES` proves nothing about what ended up on the pipe. This asks
/// the kernel what the object's DACL actually is, which is the only version of the
/// question that matters.
///
/// # Safety
///
/// `handle` must be a valid handle to a kernel object opened with
/// `READ_CONTROL` — which the creator of a named pipe always has.
#[cfg(test)]
pub unsafe fn object_dacl_sddl(handle: HANDLE) -> io::Result<String> {
    use windows_sys::Win32::Security::Authorization::{
        ConvertSecurityDescriptorToStringSecurityDescriptorW, GetSecurityInfo, SE_KERNEL_OBJECT,
    };
    use windows_sys::Win32::Security::DACL_SECURITY_INFORMATION;

    let mut psd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    let status = GetSecurityInfo(
        handle,
        SE_KERNEL_OBJECT,
        DACL_SECURITY_INFORMATION,
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        &mut psd,
    );
    // `GetSecurityInfo` returns a WIN32_ERROR rather than a BOOL.
    if status != 0 {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    // Freed via the same wrapper the SDDL path uses; `GetSecurityInfo` also
    // allocates with `LocalAlloc`.
    let owned = SecurityDescriptor(psd);

    let mut text: PWSTR = std::ptr::null_mut();
    let mut len: u32 = 0;
    let ok = ConvertSecurityDescriptorToStringSecurityDescriptorW(
        owned.0,
        SDDL_REVISION_1,
        DACL_SECURITY_INFORMATION,
        &mut text,
        &mut len,
    );
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    let sddl = wide_to_string(text);
    LocalFree(text.cast());
    Ok(sddl)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_current_user_has_a_sid_that_looks_like_one() {
        let sid = current_user_sid().expect("every process runs as somebody");
        assert!(sid.starts_with("S-1-"), "{sid}");
        // A real account SID, not a well-known alias like S-1-5-18. Tests run as
        // the developer, so this should be the interactive-user authority.
        assert!(sid.len() > 8, "{sid}");
    }

    #[test]
    fn a_descriptor_can_be_built_for_this_user() {
        let sid = current_user_sid().unwrap();
        let sd = SecurityDescriptor::owner_only(&sid).expect("valid SDDL");
        let attrs = sd.attributes();
        assert_eq!(
            attrs.nLength as usize,
            std::mem::size_of::<SECURITY_ATTRIBUTES>()
        );
        assert!(!attrs.lpSecurityDescriptor.is_null());
        assert_eq!(attrs.bInheritHandle, 0, "the pipe must not be inheritable");
    }

    #[test]
    fn nonsense_sddl_is_an_error_and_not_a_silent_default() {
        // The failure that would matter: a malformed descriptor must not quietly
        // become "no descriptor", which is what an unwrap_or_default here would
        // have made it.
        assert!(SecurityDescriptor::from_sddl("this is not SDDL").is_err());
        // A syntactically valid SDDL with a SID that does not resolve.
        assert!(SecurityDescriptor::from_sddl("D:P(A;;FA;;;S-1-5-21-not-a-sid)").is_err());
    }
}
