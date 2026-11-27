//! The socket on Windows: a named pipe that only the current user can open.

#![allow(unsafe_code)]

use std::ffi::c_void;
use std::io;
use std::sync::Arc;

use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
use tokio::sync::watch;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, LocalFree};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
    TokenUser,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

use super::{Shared, serve_connection};
use crate::diagnostics::tracing::debug;

/// The pipe, and what creating its instances needs.
pub(super) struct Listener {
    name: String,
    options: ServerOptions,
    security: Security,
    first: NamedPipeServer,
}

// The security descriptor is only read by the system, from the thread of
// the socket.
unsafe impl Send for Listener {}

impl Listener {
    /// Creates the first instance of a pipe with a name nobody else uses.
    pub(super) fn create(max_connections: usize) -> io::Result<(Self, String)> {
        let name = format!(
            r"\\.\pipe\termx-{}-{}",
            std::process::id(),
            &super::new_ticket()?[..16]
        );
        let security = Security::current_user_only()?;
        let mut options = ServerOptions::new();
        options
            .reject_remote_clients(true)
            .access_inbound(true)
            .access_outbound(true)
            // One instance waits for a client while the others serve.
            .max_instances((max_connections + 1).min(254));
        // Fails if the name exists: someone else made it first.
        options.first_pipe_instance(true);
        let first = security.create(&options, &name)?;
        options.first_pipe_instance(false);
        Ok((
            Self {
                name: name.clone(),
                options,
                security,
                first,
            },
            name,
        ))
    }

    /// Serves clients until `stopped` says so.
    pub(super) async fn serve(self, shared: Arc<Shared>, mut stopped: watch::Receiver<bool>) {
        let mut server = self.first;
        loop {
            let connected = tokio::select! {
                connected = server.connect() => connected,
                _ = stopped.changed() => return,
            };
            let connected = match connected {
                Ok(()) => true,
                Err(error) => {
                    debug!(error = %error, "socket: a client could not connect");
                    false
                }
            };
            let client = server;
            // A place for the next client before a new instance waits.
            let permit = tokio::select! {
                permit = Arc::clone(&shared.permits).acquire_owned() => permit,
                _ = stopped.changed() => return,
            };
            let Ok(permit) = permit else { return };
            server = loop {
                match self.security.create(&self.options, &self.name) {
                    Ok(server) => break server,
                    Err(error) => {
                        debug!(error = %error, "socket: a pipe instance could not be created");
                        tokio::select! {
                            () = tokio::time::sleep(std::time::Duration::from_millis(50)) => {}
                            _ = stopped.changed() => return,
                        }
                    }
                }
            };
            let shared = Arc::clone(&shared);
            let mut stop = stopped.clone();
            tokio::spawn(async move {
                if connected {
                    tokio::select! {
                        () = serve_connection(client, &shared) => {}
                        _ = stop.changed() => {}
                    }
                }
                drop(permit);
            });
        }
    }
}

/// A security descriptor that grants every access to the current user,
/// and none to anyone else.
struct Security {
    descriptor: PSECURITY_DESCRIPTOR,
}

impl Security {
    fn current_user_only() -> io::Result<Self> {
        let sid = current_user_sid()?;
        // Protected DACL, one entry: generic all, for that user.
        let sddl: Vec<u16> = format!("D:P(A;;GA;;;{sid})")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        // SAFETY: the string is terminated; the descriptor is freed in
        // `drop`.
        let converted = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &raw mut descriptor,
                std::ptr::null_mut(),
            )
        };
        if converted == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { descriptor })
    }

    fn create(&self, options: &ServerOptions, name: &str) -> io::Result<NamedPipeServer> {
        let mut attributes = SECURITY_ATTRIBUTES {
            nLength: u32::try_from(std::mem::size_of::<SECURITY_ATTRIBUTES>()).unwrap_or(0),
            lpSecurityDescriptor: self.descriptor,
            bInheritHandle: 0,
        };
        // SAFETY: the attributes point at a valid descriptor, alive for
        // the duration of the call.
        unsafe {
            options
                .create_with_security_attributes_raw(name, (&raw mut attributes).cast::<c_void>())
        }
    }
}

impl Drop for Security {
    fn drop(&mut self) {
        // SAFETY: allocated by ConvertStringSecurityDescriptorToSecurityDescriptorW.
        unsafe { LocalFree(self.descriptor.cast()) };
    }
}

/// The SID of the user of this process, as text (`S-1-5-21-...`).
fn current_user_sid() -> io::Result<String> {
    let mut token: HANDLE = std::ptr::null_mut();
    // SAFETY: plain call; the handle is closed below.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let result = (|| {
        let mut size = 0u32;
        // SAFETY: asks for the size only.
        unsafe { GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &raw mut size) };
        let mut buffer = vec![0u64; (size as usize).div_ceil(8).max(1)];
        // SAFETY: the buffer is at least `size` bytes, aligned for TOKEN_USER.
        let read = unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                buffer.as_mut_ptr().cast(),
                size,
                &raw mut size,
            )
        };
        if read == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the system wrote a TOKEN_USER at the start of the buffer.
        let user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
        let mut text: *mut u16 = std::ptr::null_mut();
        // SAFETY: the SID lives in `buffer`; the text is freed below.
        if unsafe { ConvertSidToStringSidW(user.User.Sid, &raw mut text) } == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: a terminated string allocated by the system.
        let sid = unsafe {
            let length = (0..).take_while(|&i| *text.add(i) != 0).count();
            let sid = String::from_utf16_lossy(std::slice::from_raw_parts(text, length));
            LocalFree(text.cast());
            sid
        };
        Ok(sid)
    })();
    // SAFETY: opened above.
    unsafe { CloseHandle(token) };
    result
}

#[cfg(test)]
mod tests {
    use std::os::windows::io::AsRawHandle;

    use windows_sys::Win32::Security::Authorization::{
        ConvertSecurityDescriptorToStringSecurityDescriptorW, GetSecurityInfo, SE_KERNEL_OBJECT,
    };
    use windows_sys::Win32::Security::DACL_SECURITY_INFORMATION;

    use super::*;
    use crate::socket::{Server, ServerConfig};

    /// The access list of the pipe, in SDDL.
    fn dacl_of(pipe: &std::fs::File) -> String {
        let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        // SAFETY: the handle is open; the descriptor is freed below.
        let status = unsafe {
            GetSecurityInfo(
                pipe.as_raw_handle(),
                SE_KERNEL_OBJECT,
                DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &raw mut descriptor,
            )
        };
        assert_eq!(status, 0, "GetSecurityInfo failed");
        let mut text: *mut u16 = std::ptr::null_mut();
        // SAFETY: a descriptor from the system; the text is freed below.
        let converted = unsafe {
            ConvertSecurityDescriptorToStringSecurityDescriptorW(
                descriptor,
                SDDL_REVISION_1,
                DACL_SECURITY_INFORMATION,
                &raw mut text,
                std::ptr::null_mut(),
            )
        };
        assert_ne!(converted, 0);
        // SAFETY: a terminated string allocated by the system.
        unsafe {
            let length = (0..).take_while(|&i| *text.add(i) != 0).count();
            let sddl = String::from_utf16_lossy(std::slice::from_raw_parts(text, length));
            LocalFree(text.cast());
            LocalFree(descriptor.cast());
            sddl
        }
    }

    #[test]
    fn only_the_current_user_may_open_the_pipe() {
        let server = Server::start(
            Arc::new(|_| termx_protocol::Response::ok()),
            ServerConfig::default(),
        )
        .unwrap();
        let pipe = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(server.address())
            .unwrap();

        let dacl = dacl_of(&pipe);

        let sid = current_user_sid().unwrap();
        assert!(dacl.starts_with("D:P"), "{dacl}");
        assert_eq!(dacl.matches("(A;").count(), 1, "{dacl}");
        assert!(dacl.contains(&sid), "{dacl} does not name {sid}");
        assert!(!dacl.contains(";WD)") && !dacl.contains(";AN)"), "{dacl}");
    }
}
