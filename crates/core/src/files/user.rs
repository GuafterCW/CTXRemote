//! Whose rights file operations run with.
//!
//! In service mode the agent runs as SYSTEM. A viewer must not get more than
//! the signed-in user could do at the keyboard, so the host's file thread
//! impersonates that user and refuses while nobody is signed in.

use std::path::PathBuf;

pub use imp::UserContext;

/// Display names and paths of the well-known folders, from the platform's lookup.
#[cfg(not(windows))]
fn standard_places() -> Vec<(String, PathBuf)> {
    let mut places = Vec::new();
    if let Some(dirs) = directories::UserDirs::new() {
        places.push(("Persönlicher Ordner".to_string(), dirs.home_dir().to_path_buf()));
        for (name, dir) in [
            ("Desktop", dirs.desktop_dir()),
            ("Dokumente", dirs.document_dir()),
            ("Downloads", dirs.download_dir()),
        ] {
            if let Some(dir) = dir {
                places.push((name.to_string(), dir.to_path_buf()));
            }
        }
    }
    places
}

#[cfg(not(windows))]
mod imp {
    use super::*;

    /// The rights of this process; there is no impersonation outside Windows.
    pub struct UserContext(());

    impl UserContext {
        pub fn current() -> Self {
            Self(())
        }

        pub fn for_host() -> anyhow::Result<Self> {
            Ok(Self(()))
        }

        pub fn places(&self) -> Vec<(String, PathBuf)> {
            standard_places()
        }

        /// The folder downloads go to by default.
        pub fn downloads(&self) -> Option<PathBuf> {
            directories::UserDirs::new()
                .and_then(|d| d.download_dir().map(PathBuf::from).or_else(|| Some(d.home_dir().to_path_buf())))
        }

        /// Where files pasted through the clipboard wait (see [`super::super::paste_dir`]).
        pub fn paste_root(&self) -> PathBuf {
            std::env::temp_dir().join(super::PASTE_FOLDER)
        }
    }
}

/// Below the user's temp folder.
const PASTE_FOLDER: &str = "CTXRemote-Einfuegen";

#[cfg(windows)]
mod imp {
    use std::marker::PhantomData;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};

    use anyhow::{bail, Context, Result};
    use windows::core::GUID;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Security::{
        GetTokenInformation, ImpersonateLoggedOnUser, IsWellKnownSid, RevertToSelf, TokenUser,
        WinLocalSystemSid, TOKEN_QUERY, TOKEN_USER,
    };
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::System::RemoteDesktop::{ProcessIdToSessionId, WTSQueryUserToken};
    use windows::Win32::System::Threading::{GetCurrentProcess, GetCurrentProcessId, OpenProcessToken};
    use windows::Win32::UI::Shell::{
        FOLDERID_Desktop, FOLDERID_Documents, FOLDERID_Downloads, FOLDERID_LocalAppData, FOLDERID_Profile, SHGetKnownFolderPath,
        KF_FLAG_DEFAULT,
    };

    use super::*;

    /// The process's own rights, or (as SYSTEM) the signed-in user's rights on
    /// the creating thread until dropped. Bound to that thread.
    pub struct UserContext {
        token: Option<OwnedHandle>,
        _thread_bound: PhantomData<*const ()>,
    }

    impl UserContext {
        pub fn current() -> Self {
            Self { token: None, _thread_bound: PhantomData }
        }

        /// As SYSTEM, impersonates the user signed in to this process's
        /// session; fails while nobody is signed in.
        pub fn for_host() -> Result<Self> {
            if !running_as_system()? {
                return Ok(Self::current());
            }
            let mut session = 0;
            unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut session) }?;
            let mut token = HANDLE::default();
            // Needs SeTcbPrivilege, which SYSTEM holds; fails without a signed-in user.
            if unsafe { WTSQueryUserToken(session, &mut token) }.is_err() {
                bail!("Dateien sind erst verfügbar, wenn ein Benutzer angemeldet ist");
            }
            let token = unsafe { OwnedHandle::from_raw_handle(token.0) };
            unsafe { ImpersonateLoggedOnUser(HANDLE(token.as_raw_handle())) }
                .context("Benutzerrechte konnten nicht übernommen werden")?;
            Ok(Self { token: Some(token), _thread_bound: PhantomData })
        }

        pub fn places(&self) -> Vec<(String, PathBuf)> {
            [
                ("Persönlicher Ordner", &FOLDERID_Profile),
                ("Desktop", &FOLDERID_Desktop),
                ("Dokumente", &FOLDERID_Documents),
                ("Downloads", &FOLDERID_Downloads),
            ]
            .into_iter()
            .filter_map(|(name, id)| Some((name.to_string(), self.known_folder(id)?)))
            .collect()
        }

        pub fn downloads(&self) -> Option<PathBuf> {
            self.known_folder(&FOLDERID_Downloads).or_else(|| self.known_folder(&FOLDERID_Profile))
        }

        /// Where files pasted through the clipboard wait: the user's own temp
        /// folder (as SYSTEM, `temp_dir` would be SYSTEM's).
        pub fn paste_root(&self) -> PathBuf {
            self.known_folder(&FOLDERID_LocalAppData)
                .map(|local| local.join("Temp"))
                .unwrap_or_else(std::env::temp_dir)
                .join(super::PASTE_FOLDER)
        }

        /// Resolved for the impersonated user if there is one; redirected
        /// folders (e.g. OneDrive) come out right this way.
        fn known_folder(&self, id: &GUID) -> Option<PathBuf> {
            let token = self.token.as_ref().map(|t| HANDLE(t.as_raw_handle()));
            unsafe {
                let path = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, token).ok()?;
                let result = path.to_string().ok().map(PathBuf::from);
                CoTaskMemFree(Some(path.0 as *const _));
                result
            }
        }
    }

    impl Drop for UserContext {
        fn drop(&mut self) {
            if self.token.is_some() {
                let _ = unsafe { RevertToSelf() };
            }
        }
    }

    fn running_as_system() -> Result<bool> {
        unsafe {
            let mut token = HANDLE::default();
            OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)?;
            let token = OwnedHandle::from_raw_handle(token.0);
            let mut needed = 0;
            let _ = GetTokenInformation(HANDLE(token.as_raw_handle()), TokenUser, None, 0, &mut needed);
            let mut buf = vec![0u64; (needed as usize).div_ceil(8)];
            GetTokenInformation(
                HANDLE(token.as_raw_handle()),
                TokenUser,
                Some(buf.as_mut_ptr().cast()),
                needed,
                &mut needed,
            )?;
            let user = &*(buf.as_ptr() as *const TOKEN_USER);
            Ok(IsWellKnownSid(user.User.Sid, WinLocalSystemSid).as_bool())
        }
    }
}
