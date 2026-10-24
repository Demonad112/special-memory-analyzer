//! NTFS security descriptors: the owner and the access entries (DACL) of a file or folder, decoded
//! into plain structs, plus the names of the accounts behind the SIDs.
//!
//! The decoding works on the self-relative bytes Windows hands back, so it is tested on every
//! platform with fixed bytes. Only [`read`] and the OS lookup in [`SidNames`] call Windows; they
//! report "not supported" elsewhere. [`read`] opens the path itself, never the target of a
//! reparse point, and only asks for `READ_CONTROL`, so no file data is read (an online-only file
//! stays in the cloud).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io;
use std::path::Path;
use std::sync::Mutex;

/// What to read: the owner alone is cheaper and needs no SDDL text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum What {
    Owner,
    OwnerAndDacl,
}

/// The decoded descriptor of one file or folder.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Descriptor {
    /// `S-1-5-...`; `None` when the descriptor has no owner.
    pub owner: Option<String>,
    /// The DACL does not inherit from the parent folder ("inheritance disabled").
    pub protected: bool,
    /// The DACL was read and is missing (a "null DACL"), which grants everyone full access. False
    /// when only the owner was read.
    pub null_dacl: bool,
    pub aces: Vec<Ace>,
    /// The owner and DACL as SDDL text, when the DACL was read on Windows.
    pub sddl: Option<String>,
}

/// One access entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Ace {
    pub sid: String,
    pub kind: AceKind,
    /// The raw `ACE_HEADER.AceType`, for the CSV and for kinds without their own variant.
    pub ace_type: u8,
    /// Access mask as stored (generic bits not mapped).
    pub mask: u32,
    /// Raw `ACE_HEADER.AceFlags`.
    pub flags: u8,
    /// Came from a parent folder rather than being set here.
    pub inherited: bool,
    pub applies_to: AppliesTo,
}

impl Default for Ace {
    fn default() -> Self {
        Self {
            sid: String::new(),
            kind: AceKind::Other,
            ace_type: 0,
            mask: 0,
            flags: 0,
            inherited: false,
            applies_to: AppliesTo::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AceKind {
    Allow,
    Deny,
    /// Audit, alarm, label and other types that grant or deny nothing.
    Other,
}

/// Where an entry applies, from its inheritance flags.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AppliesTo {
    /// The item itself (false for an inherit-only entry).
    pub this: bool,
    pub subfolders: bool,
    pub files: bool,
    /// Only one level down (`NO_PROPAGATE_INHERIT_ACE`).
    pub one_level: bool,
}

impl AppliesTo {
    pub fn from_flags(flags: u8) -> Self {
        Self {
            this: flags & INHERIT_ONLY_ACE == 0,
            subfolders: flags & CONTAINER_INHERIT_ACE != 0,
            files: flags & OBJECT_INHERIT_ACE != 0,
            one_level: flags & NO_PROPAGATE_INHERIT_ACE != 0,
        }
    }

    /// The words Explorer's advanced security dialog uses.
    pub fn words(self) -> &'static str {
        match (self.this, self.subfolders, self.files) {
            (true, false, false) => "This folder only",
            (true, true, true) => "This folder, subfolders and files",
            (true, true, false) => "This folder and subfolders",
            (true, false, true) => "This folder and files",
            (false, true, true) => "Subfolders and files only",
            (false, true, false) => "Subfolders only",
            (false, false, true) => "Files only",
            (false, false, false) => "Nothing",
        }
    }
}

const OBJECT_INHERIT_ACE: u8 = 0x1;
const CONTAINER_INHERIT_ACE: u8 = 0x2;
const NO_PROPAGATE_INHERIT_ACE: u8 = 0x4;
const INHERIT_ONLY_ACE: u8 = 0x8;
const INHERITED_ACE: u8 = 0x10;

const SE_DACL_PRESENT: u16 = 0x0004;
const SE_DACL_PROTECTED: u16 = 0x1000;
const SE_SELF_RELATIVE: u16 = 0x8000;

// File access rights (winnt.h).
pub const FILE_READ_DATA: u32 = 0x1;
pub const FILE_WRITE_DATA: u32 = 0x2;
pub const FILE_APPEND_DATA: u32 = 0x4;
pub const FILE_EXECUTE: u32 = 0x20;
pub const DELETE: u32 = 0x1_0000;
pub const WRITE_DAC: u32 = 0x4_0000;
pub const WRITE_OWNER: u32 = 0x8_0000;
pub const GENERIC_ALL: u32 = 0x1000_0000;
pub const GENERIC_EXECUTE: u32 = 0x2000_0000;
pub const GENERIC_WRITE: u32 = 0x4000_0000;
pub const GENERIC_READ: u32 = 0x8000_0000;
/// `FILE_ALL_ACCESS`: what Explorer calls Full control.
pub const FULL_CONTROL: u32 = 0x1F_01FF;
/// Explorer's Modify: read, write, execute and delete.
pub const MODIFY: u32 = 0x13_01BF;
pub const READ_AND_EXECUTE: u32 = 0x12_00A9;
pub const READ: u32 = 0x12_0089;
pub const WRITE: u32 = 0x10_0116;
const FILE_GENERIC_READ: u32 = 0x12_0089;
const FILE_GENERIC_WRITE: u32 = 0x12_0116;
const FILE_GENERIC_EXECUTE: u32 = 0x12_00A0;

/// Rights that let a principal change or remove data or the permissions themselves (D-06): write
/// data, append, delete, write DAC, write owner, generic write, generic all.
const WIDE_WRITE_BITS: u32 = FILE_WRITE_DATA
    | FILE_APPEND_DATA
    | DELETE
    | WRITE_DAC
    | WRITE_OWNER
    | GENERIC_WRITE
    | GENERIC_ALL;

/// True when `mask` includes any right D-06 counts as write access. Read and execute are not.
pub fn is_wide_write(mask: u32) -> bool {
    mask & WIDE_WRITE_BITS != 0
}

/// The access level of a mask in Explorer's words.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Rights {
    FullControl,
    Modify,
    ReadWriteAndExecute,
    ReadAndWrite,
    ReadAndExecute,
    Read,
    Write,
    /// Any other combination; the raw mask tells which.
    Special,
}

impl Rights {
    pub fn from_mask(mask: u32) -> Self {
        let mask = map_generic(mask);
        let has = |bits: u32| mask & bits == bits;
        let read = has(READ & 0xFFFF);
        let write = has(WRITE & 0xFFFF);
        let execute = has(FILE_EXECUTE);
        if has(FULL_CONTROL) {
            Self::FullControl
        } else if has(MODIFY) {
            Self::Modify
        } else if read && write && execute {
            Self::ReadWriteAndExecute
        } else if read && write {
            Self::ReadAndWrite
        } else if read && execute {
            Self::ReadAndExecute
        } else if read {
            Self::Read
        } else if write {
            Self::Write
        } else {
            Self::Special
        }
    }

    pub fn words(self) -> &'static str {
        match self {
            Self::FullControl => "Full control",
            Self::Modify => "Modify",
            Self::ReadWriteAndExecute => "Read, write and execute",
            Self::ReadAndWrite => "Read and write",
            Self::ReadAndExecute => "Read and execute",
            Self::Read => "Read",
            Self::Write => "Write",
            Self::Special => "Special",
        }
    }
}

/// Generic rights spelled out as the file rights they stand for.
fn map_generic(mask: u32) -> u32 {
    let mut mapped = mask & !(GENERIC_ALL | GENERIC_EXECUTE | GENERIC_WRITE | GENERIC_READ);
    if mask & GENERIC_ALL != 0 {
        mapped |= FULL_CONTROL;
    }
    if mask & GENERIC_READ != 0 {
        mapped |= FILE_GENERIC_READ;
    }
    if mask & GENERIC_WRITE != 0 {
        mapped |= FILE_GENERIC_WRITE;
    }
    if mask & GENERIC_EXECUTE != 0 {
        mapped |= FILE_GENERIC_EXECUTE;
    }
    mapped
}

/// Principals auditors ask about when they hold write access (D-06).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WellKnown {
    Everyone,
    AuthenticatedUsers,
    /// `BUILTIN\Users`.
    Users,
    /// A domain's Domain Users group (RID 513).
    DomainUsers,
    Anonymous,
}

impl WellKnown {
    pub fn words(self) -> &'static str {
        match self {
            Self::Everyone => "Everyone",
            Self::AuthenticatedUsers => "Authenticated Users",
            Self::Users => "BUILTIN\\Users",
            Self::DomainUsers => "Domain Users",
            Self::Anonymous => "Anonymous",
        }
    }
}

/// The risky principal a SID stands for, if any.
pub fn well_known(sid: &str) -> Option<WellKnown> {
    let sid = sid.to_ascii_uppercase();
    match sid.as_str() {
        "S-1-1-0" => Some(WellKnown::Everyone),
        "S-1-5-11" => Some(WellKnown::AuthenticatedUsers),
        "S-1-5-32-545" => Some(WellKnown::Users),
        "S-1-5-7" => Some(WellKnown::Anonymous),
        _ => {
            // S-1-5-21-<three domain parts>-513
            let parts: Vec<&str> = sid.split('-').collect();
            let domain_users = parts.len() == 8
                && parts[..4] == ["S", "1", "5", "21"]
                && parts[7] == "513"
                && parts[4..7].iter().all(|part| part.parse::<u32>().is_ok());
            domain_users.then_some(WellKnown::DomainUsers)
        }
    }
}

/// An allow entry that gives a risky principal write access.
pub fn is_wide_write_grant(ace: &Ace) -> Option<WellKnown> {
    if ace.kind != AceKind::Allow || !is_wide_write(ace.mask) {
        return None;
    }
    well_known(&ace.sid)
}

/// Decodes a self-relative security descriptor (`SECURITY_DESCRIPTOR_RELATIVE`). `dacl_read`
/// says whether the DACL was asked for, so a missing one means a null DACL. Never panics on
/// malformed bytes; they are an `InvalidData` error.
pub fn decode(bytes: &[u8], dacl_read: bool) -> io::Result<Descriptor> {
    let control = read_u16(bytes, 2)?;
    if read_u8(bytes, 0)? != 1 || control & SE_SELF_RELATIVE == 0 {
        return Err(invalid("not a self-relative security descriptor"));
    }
    let owner_at = read_u32(bytes, 4)? as usize;
    let dacl_at = read_u32(bytes, 16)? as usize;
    let owner = if owner_at == 0 {
        None
    } else {
        Some(decode_sid(bytes, owner_at)?.0)
    };
    let dacl_present = control & SE_DACL_PRESENT != 0 && dacl_at != 0;
    let aces = if dacl_present {
        decode_acl(bytes, dacl_at)?
    } else {
        Vec::new()
    };
    Ok(Descriptor {
        owner,
        protected: control & SE_DACL_PROTECTED != 0,
        null_dacl: dacl_read && !dacl_present,
        aces,
        sddl: None,
    })
}

fn decode_acl(bytes: &[u8], at: usize) -> io::Result<Vec<Ace>> {
    let size = read_u16(bytes, at + 2)? as usize;
    let count = read_u16(bytes, at + 4)? as usize;
    let end = at
        .checked_add(size)
        .filter(|&end| end <= bytes.len())
        .ok_or_else(|| invalid("the access list runs past the descriptor"))?;
    let acl = &bytes[..end];
    let mut aces = Vec::with_capacity(count.min(1024));
    let mut offset = at + 8;
    for _ in 0..count {
        let ace_type = read_u8(acl, offset)?;
        let flags = read_u8(acl, offset + 1)?;
        let ace_size = read_u16(acl, offset + 2)? as usize;
        if ace_size < 8 || offset + ace_size > acl.len() {
            return Err(invalid("an access entry runs past its list"));
        }
        let ace = &acl[..offset + ace_size];
        let mask = read_u32(ace, offset + 4)?;
        let (kind, sid_at) = match ace_type {
            // ACCESS_ALLOWED, ACCESS_DENIED, and their callback (conditional) forms.
            0 | 9 => (AceKind::Allow, Some(offset + 8)),
            1 | 10 => (AceKind::Deny, Some(offset + 8)),
            // Object forms: mask, flags, then up to two GUIDs before the SID.
            5 | 6 | 11 | 12 => {
                let object_flags = read_u32(ace, offset + 8)?;
                let guids = (object_flags & 1) + ((object_flags >> 1) & 1);
                let kind = if matches!(ace_type, 5 | 11) {
                    AceKind::Allow
                } else {
                    AceKind::Deny
                };
                (kind, Some(offset + 12 + 16 * guids as usize))
            }
            // Audit, alarm, mandatory label and the rest keep the SID after the mask too.
            _ => (AceKind::Other, Some(offset + 8)),
        };
        let sid = match sid_at {
            Some(sid_at) => decode_sid(ace, sid_at)?.0,
            None => String::new(),
        };
        aces.push(Ace {
            sid,
            kind,
            ace_type,
            mask,
            flags,
            inherited: flags & INHERITED_ACE != 0,
            applies_to: AppliesTo::from_flags(flags),
        });
        offset += ace_size;
    }
    Ok(aces)
}

/// `S-R-A-S1-S2...` text of the SID at `at`, and its length in bytes.
pub fn decode_sid(bytes: &[u8], at: usize) -> io::Result<(String, usize)> {
    let revision = read_u8(bytes, at)?;
    let count = read_u8(bytes, at + 1)? as usize;
    if revision != 1 || count > 15 {
        return Err(invalid("not a SID"));
    }
    let authority_bytes = bytes
        .get(at + 2..at + 8)
        .ok_or_else(|| invalid("a SID runs past the descriptor"))?;
    let authority = authority_bytes
        .iter()
        .fold(0_u64, |value, byte| (value << 8) | u64::from(*byte));
    let mut text = if authority >> 32 == 0 {
        format!("S-{revision}-{authority}")
    } else {
        format!("S-{revision}-0x{authority:012X}")
    };
    for index in 0..count {
        text.push('-');
        text.push_str(&read_u32(bytes, at + 8 + 4 * index)?.to_string());
    }
    Ok((text, 8 + 4 * count))
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("Bad security descriptor: {message}"),
    )
}

fn read_u8(bytes: &[u8], at: usize) -> io::Result<u8> {
    bytes
        .get(at)
        .copied()
        .ok_or_else(|| invalid("it ends too early"))
}

fn read_u16(bytes: &[u8], at: usize) -> io::Result<u16> {
    bytes
        .get(at..at.saturating_add(2))
        .map(|raw| u16::from_le_bytes([raw[0], raw[1]]))
        .ok_or_else(|| invalid("it ends too early"))
}

fn read_u32(bytes: &[u8], at: usize) -> io::Result<u32> {
    bytes
        .get(at..at.saturating_add(4))
        .map(|raw| u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]))
        .ok_or_else(|| invalid("it ends too early"))
}

/// Reads the owner (and the DACL) of `path` without following a reparse point: a link or junction
/// reports its own descriptor, not its target's.
pub fn read(path: &Path, what: What) -> io::Result<Descriptor> {
    imp::read(path, what)
}

/// What an account lookup found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "result")]
pub enum Account {
    Found {
        name: String,
        domain: String,
    },
    /// No account has this SID any more (`ERROR_NONE_MAPPED`): an orphaned entry.
    NotFound,
    /// The lookup failed for another reason (the server could not be reached, ...).
    Failed {
        code: i32,
    },
    /// Not on this system (lookups need Windows).
    NotSupported,
}

impl Account {
    /// `DOMAIN\name`, `name` without a domain, or the SID with a note.
    pub fn display(&self, sid: &str) -> String {
        match self {
            Self::Found { name, domain } if domain.is_empty() => name.clone(),
            Self::Found { name, domain } => format!("{domain}\\{name}"),
            Self::NotFound => format!("{sid} (account not found)"),
            Self::Failed { .. } | Self::NotSupported => sid.to_owned(),
        }
    }
}

type Resolver = dyn Fn(&str, Option<&str>) -> Account + Send + Sync;

/// Account names for SIDs, each looked up once per server and kept for the run. Safe to share
/// between the threads of a parallel walk.
pub struct SidNames {
    resolve: Box<Resolver>,
    cache: Mutex<HashMap<(String, String), Account>>,
}

impl Default for SidNames {
    fn default() -> Self {
        Self::with_resolver(imp::lookup)
    }
}

impl SidNames {
    /// A cache in front of `resolve(sid, server)` instead of the system lookup (tests).
    pub fn with_resolver(
        resolve: impl Fn(&str, Option<&str>) -> Account + Send + Sync + 'static,
    ) -> Self {
        Self {
            resolve: Box::new(resolve),
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// The account for `sid`, asked of `server` (`None` for this machine). For a path on a share,
    /// pass the server: its local groups only resolve there (D-07).
    pub fn lookup(&self, sid: &str, server: Option<&str>) -> Account {
        let key = (
            server.unwrap_or_default().to_lowercase(),
            sid.to_ascii_uppercase(),
        );
        if let Some(found) = self.lock().get(&key) {
            return found.clone();
        }
        // Not under the lock: a lookup can take seconds when a domain controller is slow.
        let found = (self.resolve)(sid, server);
        self.lock().entry(key).or_insert(found).clone()
    }

    /// How many SIDs are cached.
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<(String, String), Account>> {
        self.cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(windows)]
mod imp {
    use super::{decode, Account, Descriptor, What};
    use crate::list::wide_path;
    use std::io;
    use std::path::Path;
    use std::ptr;
    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, LocalFree, ERROR_INSUFFICIENT_BUFFER, ERROR_NONE_MAPPED, HANDLE,
        INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Security::Authorization::{
        ConvertSecurityDescriptorToStringSecurityDescriptorW, ConvertStringSidToSidW,
    };
    use windows_sys::Win32::Security::{GetKernelObjectSecurity, LookupAccountSidW, SID_NAME_USE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE,
        FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };

    const READ_CONTROL: u32 = 0x2_0000;
    const OWNER_SECURITY_INFORMATION: u32 = 0x1;
    const DACL_SECURITY_INFORMATION: u32 = 0x4;
    const SDDL_REVISION_1: u32 = 1;

    struct Handle(HANDLE);

    impl Drop for Handle {
        fn drop(&mut self) {
            // SAFETY: the handle came from CreateFileW and is closed once.
            unsafe { CloseHandle(self.0) };
        }
    }

    fn wide_str(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(Some(0)).collect()
    }

    /// Text from a NUL-terminated wide string that LocalFree owns; frees it.
    ///
    /// SAFETY: `text` must be a live, NUL-terminated buffer from a LocalAlloc-ing API.
    unsafe fn take_local_text(text: *mut u16) -> String {
        let length = (0..).take_while(|&i| *text.add(i) != 0).count();
        let result = String::from_utf16_lossy(std::slice::from_raw_parts(text, length));
        LocalFree(text.cast());
        result
    }

    pub fn read(path: &Path, what: What) -> io::Result<Descriptor> {
        let wide = wide_path(path);
        // READ_CONTROL only: no data access, so nothing is recalled from the cloud. Backup
        // semantics are what let a folder be opened at all; OPEN_REPARSE_POINT keeps a link
        // from being followed.
        // SAFETY: `wide` is NUL-terminated; the other arguments are plain values.
        let raw = unsafe {
            CreateFileW(
                wide.as_ptr(),
                READ_CONTROL,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
                ptr::null_mut(),
            )
        };
        if raw == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let handle = Handle(raw);
        let info = match what {
            What::Owner => OWNER_SECURITY_INFORMATION,
            What::OwnerAndDacl => OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
        };
        let mut needed = 0u32;
        // u64 elements keep the buffer aligned for the descriptor.
        let mut buffer: Vec<u64> = Vec::new();
        loop {
            let length = u32::try_from(buffer.len() * 8).unwrap_or(u32::MAX);
            // SAFETY: the buffer holds `length` bytes and outlives the call.
            let ok = unsafe {
                GetKernelObjectSecurity(
                    handle.0,
                    info,
                    buffer.as_mut_ptr().cast(),
                    length,
                    &mut needed,
                )
            };
            if ok != 0 {
                break;
            }
            // SAFETY: reads the calling thread's last error.
            if unsafe { GetLastError() } != ERROR_INSUFFICIENT_BUFFER || needed <= length {
                return Err(io::Error::last_os_error());
            }
            buffer = vec![0u64; (needed as usize).div_ceil(8)];
        }
        // SAFETY: the call above wrote `needed` bytes (at most the buffer's size).
        let bytes = unsafe {
            std::slice::from_raw_parts(
                buffer.as_ptr().cast::<u8>(),
                (needed as usize).min(buffer.len() * 8),
            )
        };
        let mut descriptor = decode(bytes, what == What::OwnerAndDacl)?;
        if what == What::OwnerAndDacl {
            let mut text: *mut u16 = ptr::null_mut();
            // SAFETY: `buffer` holds a valid self-relative descriptor; `text` is freed below.
            let ok = unsafe {
                ConvertSecurityDescriptorToStringSecurityDescriptorW(
                    buffer.as_ptr().cast_mut().cast(),
                    SDDL_REVISION_1,
                    info,
                    &mut text,
                    ptr::null_mut(),
                )
            };
            if ok != 0 && !text.is_null() {
                // SAFETY: `text` came from the call above.
                descriptor.sddl = Some(unsafe { take_local_text(text) });
            }
        }
        Ok(descriptor)
    }

    pub fn lookup(sid: &str, server: Option<&str>) -> Account {
        let wide_sid = wide_str(sid);
        let mut binary = ptr::null_mut();
        // SAFETY: `wide_sid` is NUL-terminated; `binary` is freed below.
        if unsafe { ConvertStringSidToSidW(wide_sid.as_ptr(), &mut binary) } == 0 {
            return failed();
        }
        let server = server.map(wide_str);
        let server_ptr = server.as_ref().map_or(ptr::null(), |name| name.as_ptr());
        let (mut name_len, mut domain_len) = (0u32, 0u32);
        let mut kind: SID_NAME_USE = 0;
        // SAFETY: null buffers with zero lengths only ask for the sizes.
        let sized = unsafe {
            LookupAccountSidW(
                server_ptr,
                binary,
                ptr::null_mut(),
                &mut name_len,
                ptr::null_mut(),
                &mut domain_len,
                &mut kind,
            )
        };
        // SAFETY: reads the calling thread's last error.
        let error = unsafe { GetLastError() };
        let result = if sized == 0 && error == ERROR_NONE_MAPPED {
            Account::NotFound
        } else if sized == 0 && error != ERROR_INSUFFICIENT_BUFFER {
            failed()
        } else {
            let mut name = vec![0u16; name_len.max(1) as usize];
            let mut domain = vec![0u16; domain_len.max(1) as usize];
            // SAFETY: both buffers hold the lengths passed with them.
            let ok = unsafe {
                LookupAccountSidW(
                    server_ptr,
                    binary,
                    name.as_mut_ptr(),
                    &mut name_len,
                    domain.as_mut_ptr(),
                    &mut domain_len,
                    &mut kind,
                )
            };
            if ok == 0 {
                // SAFETY: reads the calling thread's last error.
                if unsafe { GetLastError() } == ERROR_NONE_MAPPED {
                    Account::NotFound
                } else {
                    failed()
                }
            } else {
                Account::Found {
                    name: String::from_utf16_lossy(&name[..name_len as usize]),
                    domain: String::from_utf16_lossy(&domain[..domain_len as usize]),
                }
            }
        };
        // SAFETY: `binary` came from ConvertStringSidToSidW.
        unsafe { LocalFree(binary) };
        result
    }

    fn failed() -> Account {
        Account::Failed {
            code: io::Error::last_os_error().raw_os_error().unwrap_or(-1),
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::{Account, Descriptor, What};
    use std::io;
    use std::path::Path;

    pub fn read(_path: &Path, _what: What) -> io::Result<Descriptor> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Permissions can only be read on Windows",
        ))
    }

    pub fn lookup(_sid: &str, _server: Option<&str>) -> Account {
        Account::NotSupported
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// Binary form of a SID string such as `S-1-5-32-544` (decimal authority only).
    fn sid_bytes(text: &str) -> Vec<u8> {
        let parts: Vec<u64> = text[2..].split('-').map(|p| p.parse().unwrap()).collect();
        let mut bytes = vec![parts[0] as u8, (parts.len() - 2) as u8];
        bytes.extend_from_slice(&parts[1].to_be_bytes()[2..]);
        for sub in &parts[2..] {
            bytes.extend_from_slice(&(*sub as u32).to_le_bytes());
        }
        bytes
    }

    fn ace_bytes(ace_type: u8, flags: u8, mask: u32, sid: &str) -> Vec<u8> {
        let sid = sid_bytes(sid);
        let size = (8 + sid.len()) as u16;
        let mut bytes = vec![ace_type, flags];
        bytes.extend_from_slice(&size.to_le_bytes());
        bytes.extend_from_slice(&mask.to_le_bytes());
        bytes.extend_from_slice(&sid);
        bytes
    }

    fn acl_bytes(aces: &[Vec<u8>]) -> Vec<u8> {
        let body: Vec<u8> = aces.concat();
        let mut bytes = vec![2, 0];
        bytes.extend_from_slice(&((8 + body.len()) as u16).to_le_bytes());
        bytes.extend_from_slice(&(aces.len() as u16).to_le_bytes());
        bytes.extend_from_slice(&[0, 0]);
        bytes.extend_from_slice(&body);
        bytes
    }

    /// A self-relative descriptor: header, then the owner SID, then the DACL.
    fn descriptor_bytes(control: u16, owner: Option<&str>, dacl: Option<Vec<u8>>) -> Vec<u8> {
        let owner = owner.map(sid_bytes);
        let mut bytes = vec![1, 0];
        bytes.extend_from_slice(&(control | SE_SELF_RELATIVE).to_le_bytes());
        let owner_at = owner.as_ref().map_or(0, |_| 20u32);
        let dacl_at = dacl
            .as_ref()
            .map_or(0, |_| 20 + owner.as_ref().map_or(0, Vec::len) as u32);
        for offset in [owner_at, 0, 0, dacl_at] {
            bytes.extend_from_slice(&offset.to_le_bytes());
        }
        bytes.extend(owner.unwrap_or_default());
        bytes.extend(dacl.unwrap_or_default());
        bytes
    }

    #[test]
    fn a_hand_written_descriptor_decodes() {
        // Owner BUILTIN\Administrators, protected DACL with one entry: Everyone, full control,
        // this folder, subfolders and files. Written out byte by byte from the winnt.h layouts.
        #[rustfmt::skip]
        let bytes: [u8; 64] = [
            1, 0, 0x04, 0x90,            // revision, sbz1, control = DACL_PRESENT|PROTECTED|SELF_RELATIVE
            20, 0, 0, 0,                 // owner at 20
            0, 0, 0, 0,                  // no group
            0, 0, 0, 0,                  // no SACL
            36, 0, 0, 0,                 // DACL at 36
            1, 2, 0, 0, 0, 0, 0, 5,      // S-1-5
            32, 0, 0, 0, 0x20, 2, 0, 0,  // -32-544
            2, 0, 28, 0, 1, 0, 0, 0,     // ACL revision 2, size 28, 1 entry
            0, 3, 20, 0,                 // ACCESS_ALLOWED, OI|CI, size 20
            0xFF, 0x01, 0x1F, 0x00,      // mask 0x001F01FF
            1, 1, 0, 0, 0, 0, 0, 1,      // S-1-1
            0, 0, 0, 0,                  // -0
        ];
        let descriptor = decode(&bytes, true).unwrap();
        assert_eq!(descriptor.owner.as_deref(), Some("S-1-5-32-544"));
        assert!(descriptor.protected);
        assert!(!descriptor.null_dacl);
        assert_eq!(
            descriptor.aces,
            vec![Ace {
                sid: "S-1-1-0".to_owned(),
                kind: AceKind::Allow,
                ace_type: 0,
                mask: FULL_CONTROL,
                flags: 3,
                inherited: false,
                applies_to: AppliesTo {
                    this: true,
                    subfolders: true,
                    files: true,
                    one_level: false
                },
            }]
        );
        assert_eq!(
            Rights::from_mask(descriptor.aces[0].mask),
            Rights::FullControl
        );
        assert_eq!(
            is_wide_write_grant(&descriptor.aces[0]),
            Some(WellKnown::Everyone)
        );
    }

    #[test]
    fn allow_deny_inherited_and_applies_to() {
        let dacl = acl_bytes(&[
            ace_bytes(1, 0, WRITE, "S-1-5-21-1-2-3-1001"),
            ace_bytes(
                0,
                INHERITED_ACE | OBJECT_INHERIT_ACE | INHERIT_ONLY_ACE,
                READ,
                "S-1-5-11",
            ),
            ace_bytes(
                0,
                CONTAINER_INHERIT_ACE | NO_PROPAGATE_INHERIT_ACE,
                MODIFY,
                "S-1-5-32-545",
            ),
        ]);
        let bytes = descriptor_bytes(SE_DACL_PRESENT, Some("S-1-5-21-1-2-3-500"), Some(dacl));
        let descriptor = decode(&bytes, true).unwrap();

        assert!(!descriptor.protected);
        assert_eq!(descriptor.owner.as_deref(), Some("S-1-5-21-1-2-3-500"));
        let [deny, inherited, explicit] = descriptor.aces.as_slice() else {
            panic!("{:?}", descriptor.aces)
        };
        assert_eq!((deny.kind, deny.inherited), (AceKind::Deny, false));
        assert_eq!(deny.applies_to.words(), "This folder only");
        assert_eq!(
            (inherited.kind, inherited.inherited),
            (AceKind::Allow, true)
        );
        assert_eq!(inherited.applies_to.words(), "Files only");
        assert!(explicit.applies_to.one_level);
        assert_eq!(explicit.applies_to.words(), "This folder and subfolders");
        assert_eq!(Rights::from_mask(explicit.mask), Rights::Modify);
    }

    #[test]
    fn object_and_other_entry_types() {
        // ACCESS_ALLOWED_OBJECT with one GUID before the SID.
        let sid = sid_bytes("S-1-1-0");
        let mut object = vec![5, 0];
        object.extend_from_slice(&((12 + 16 + sid.len()) as u16).to_le_bytes());
        object.extend_from_slice(&READ.to_le_bytes());
        object.extend_from_slice(&1u32.to_le_bytes());
        object.extend_from_slice(&[0xAB; 16]);
        object.extend_from_slice(&sid);
        // SYSTEM_MANDATORY_LABEL (0x11): grants nothing.
        let label = ace_bytes(0x11, 0, 0x1, "S-1-16-12288");
        let bytes = descriptor_bytes(SE_DACL_PRESENT, None, Some(acl_bytes(&[object, label])));

        let descriptor = decode(&bytes, true).unwrap();
        assert_eq!(descriptor.owner, None);
        assert_eq!(descriptor.aces[0].kind, AceKind::Allow);
        assert_eq!(descriptor.aces[0].sid, "S-1-1-0");
        assert_eq!(descriptor.aces[1].kind, AceKind::Other);
        assert_eq!(descriptor.aces[1].sid, "S-1-16-12288");
    }

    #[test]
    fn a_missing_dacl_is_a_null_dacl_only_when_it_was_read() {
        let bytes = descriptor_bytes(0, Some("S-1-5-18"), None);
        assert!(decode(&bytes, true).unwrap().null_dacl);
        assert!(!decode(&bytes, false).unwrap().null_dacl);
        assert!(decode(&bytes, false).unwrap().aces.is_empty());
    }

    #[test]
    fn malformed_bytes_are_errors_not_panics() {
        let good = descriptor_bytes(
            SE_DACL_PRESENT,
            Some("S-1-5-18"),
            Some(acl_bytes(&[ace_bytes(0, 0, READ, "S-1-1-0")])),
        );
        assert!(decode(&good, true).is_ok());
        for length in 0..good.len() {
            let error = decode(&good[..length], true).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidData, "{length}");
        }
        let mut absolute = good.clone();
        absolute[3] &= 0x7F; // not self-relative
        assert!(decode(&absolute, true).is_err());
        let mut huge_count = good.clone();
        let dacl_at = u32::from_le_bytes(good[16..20].try_into().unwrap()) as usize;
        huge_count[dacl_at + 4] = 0xFF;
        assert!(decode(&huge_count, true).is_err());
        let mut bad_sid = good;
        bad_sid[21] = 200; // owner sub-authority count
        assert!(decode(&bad_sid, true).is_err());
    }

    #[test]
    fn sids_with_a_large_authority_are_hex() {
        let mut bytes = vec![1, 1, 0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC];
        bytes.extend_from_slice(&7u32.to_le_bytes());
        assert_eq!(
            decode_sid(&bytes, 0).unwrap(),
            ("S-1-0x123456789ABC-7".to_owned(), 12)
        );
    }

    #[test]
    fn rights_levels_and_wide_write() {
        // Read access is normal on shares; anything that writes is flagged.
        for (mask, rights, wide) in [
            (FULL_CONTROL, Rights::FullControl, true),
            (MODIFY, Rights::Modify, true),
            (READ_AND_EXECUTE, Rights::ReadAndExecute, false),
            (READ, Rights::Read, false),
            (WRITE, Rights::Write, true),
            (READ | WRITE, Rights::ReadAndWrite, true),
            (READ_AND_EXECUTE | WRITE, Rights::ReadWriteAndExecute, true),
            (GENERIC_ALL, Rights::FullControl, true),
            (GENERIC_READ, Rights::Read, false),
            (
                GENERIC_READ | GENERIC_EXECUTE,
                Rights::ReadAndExecute,
                false,
            ),
            (GENERIC_WRITE, Rights::Write, true),
            (FILE_APPEND_DATA, Rights::Special, true),
            (DELETE, Rights::Special, true),
            (WRITE_DAC, Rights::Special, true),
            (WRITE_OWNER, Rights::Special, true),
            (0x0010_0000, Rights::Special, false), // SYNCHRONIZE alone
        ] {
            assert_eq!(Rights::from_mask(mask), rights, "{mask:#x}");
            assert_eq!(is_wide_write(mask), wide, "{mask:#x}");
        }
        assert_eq!(Rights::Modify.words(), "Modify");
    }

    #[test]
    fn risky_principals() {
        for (sid, known) in [
            ("S-1-1-0", WellKnown::Everyone),
            ("S-1-5-11", WellKnown::AuthenticatedUsers),
            ("S-1-5-32-545", WellKnown::Users),
            ("s-1-5-32-545", WellKnown::Users),
            ("S-1-5-7", WellKnown::Anonymous),
            (
                "S-1-5-21-1004336348-1177238915-682003330-513",
                WellKnown::DomainUsers,
            ),
        ] {
            assert_eq!(well_known(sid), Some(known), "{sid}");
        }
        for sid in [
            "S-1-5-32-544",         // Administrators
            "S-1-5-18",             // SYSTEM
            "S-1-5-21-1-2-3-512",   // Domain Admins
            "S-1-5-21-1-2-3-1001",  // a user
            "S-1-5-21-1-2-513",     // too few domain parts
            "S-1-5-21-1-2-3-4-513", // too many
        ] {
            assert_eq!(well_known(sid), None, "{sid}");
        }
    }

    #[test]
    fn wide_write_needs_an_allow_entry_for_a_risky_principal() {
        let ace = |kind, mask, sid: &str| Ace {
            sid: sid.to_owned(),
            kind,
            mask,
            ..Ace::default()
        };
        assert_eq!(
            is_wide_write_grant(&ace(AceKind::Allow, MODIFY, "S-1-1-0")),
            Some(WellKnown::Everyone)
        );
        assert_eq!(
            is_wide_write_grant(&ace(AceKind::Allow, READ, "S-1-1-0")),
            None
        );
        assert_eq!(
            is_wide_write_grant(&ace(AceKind::Deny, MODIFY, "S-1-1-0")),
            None
        );
        assert_eq!(
            is_wide_write_grant(&ace(AceKind::Allow, FULL_CONTROL, "S-1-5-32-544")),
            None
        );
    }

    #[test]
    fn sid_names_are_looked_up_once_per_server() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&calls);
        let names = SidNames::with_resolver(move |sid, server| {
            counter.fetch_add(1, Ordering::SeqCst);
            match (sid, server) {
                ("S-1-5-21-1-2-3-1001", _) => Account::NotFound,
                (_, Some(server)) => Account::Found {
                    name: "Staff".to_owned(),
                    domain: server.to_uppercase(),
                },
                _ => Account::Found {
                    name: "Everyone".to_owned(),
                    domain: String::new(),
                },
            }
        });

        assert_eq!(names.lookup("S-1-1-0", None).display("S-1-1-0"), "Everyone");
        assert_eq!(names.lookup("s-1-1-0", None).display("S-1-1-0"), "Everyone");
        assert_eq!(calls.load(Ordering::SeqCst), 1, "a hit, whatever the case");

        let local_group = "S-1-5-21-9-9-9-1010";
        assert_eq!(
            names.lookup(local_group, Some("fs01")).display(local_group),
            "FS01\\Staff"
        );
        names.lookup(local_group, Some("FS01"));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        names.lookup(local_group, None);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            3,
            "another machine is another entry"
        );

        let orphan = "S-1-5-21-1-2-3-1001";
        assert_eq!(
            names.lookup(orphan, None).display(orphan),
            "S-1-5-21-1-2-3-1001 (account not found)"
        );
        names.lookup(orphan, None);
        assert_eq!(calls.load(Ordering::SeqCst), 4);
        assert_eq!(names.len(), 4);
    }

    #[cfg(not(windows))]
    #[test]
    fn reading_is_not_supported_off_windows() {
        let dir = tempfile::tempdir().unwrap();
        let error = read(dir.path(), What::OwnerAndDacl).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Unsupported);
        assert_eq!(
            SidNames::default().lookup("S-1-1-0", None),
            Account::NotSupported
        );
    }

    #[cfg(windows)]
    mod windows {
        use super::super::*;
        use std::path::Path;

        /// Owners new files can get: the user's SID (from `whoami`), or Administrators when the
        /// test runs as an elevated administrator (that token's default owner).
        fn expected_owners() -> [String; 2] {
            let output = std::process::Command::new("whoami")
                .args(["/user", "/fo", "csv", "/nh"])
                .output()
                .unwrap();
            let text = String::from_utf8_lossy(&output.stdout);
            let sid = text.trim().rsplit(',').next().unwrap().trim_matches('"');
            assert!(sid.starts_with("S-1-"), "{text}");
            [sid.to_owned(), "S-1-5-32-544".to_owned()]
        }

        fn mklink_junction(link: &Path, target: &Path) {
            let output = std::process::Command::new("cmd")
                .args(["/C", "mklink", "/J"])
                .arg(link)
                .arg(target)
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
        }

        #[test]
        fn a_temp_folder_is_owned_by_the_test_user() {
            let dir = tempfile::tempdir().unwrap();
            let folder = dir.path().join("made");
            std::fs::create_dir(&folder).unwrap();
            std::fs::write(folder.join("f.txt"), b"x").unwrap();

            let descriptor = read(&folder, What::OwnerAndDacl).unwrap();
            let owners = expected_owners();
            let owner = descriptor.owner.clone().unwrap();
            assert!(owners.contains(&owner), "{owner} not in {owners:?}");
            assert!(!descriptor.protected);
            assert!(!descriptor.null_dacl);
            // Whether the entries are inherited depends on the temp folder's own ACL, so check the
            // decoding against Windows' SDDL text entry by entry instead: `(A;OICI;FA;;;SY)`.
            let sddl = descriptor.sddl.clone().unwrap();
            assert!(sddl.starts_with("O:"), "{sddl}");
            let dacl = &sddl[sddl.find("D:").unwrap()..];
            let entries: Vec<Vec<&str>> = dacl
                .split('(')
                .skip(1)
                .map(|entry| entry.trim_end_matches(')').split(';').collect())
                .collect();
            assert!(!entries.is_empty(), "{sddl}");
            assert_eq!(entries.len(), descriptor.aces.len(), "{sddl}");
            for (entry, ace) in entries.iter().zip(&descriptor.aces) {
                let kind = match entry[0] {
                    "A" => AceKind::Allow,
                    "D" => AceKind::Deny,
                    _ => AceKind::Other,
                };
                assert_eq!(ace.kind, kind, "{sddl}");
                assert_eq!(ace.inherited, entry[1].contains("ID"), "{sddl}");
                assert_eq!(ace.applies_to.subfolders, entry[1].contains("CI"), "{sddl}");
                assert_eq!(ace.applies_to.files, entry[1].contains("OI"), "{sddl}");
            }

            let file = read(&folder.join("f.txt"), What::Owner).unwrap();
            assert_eq!(file.owner, Some(owner));
            assert!(file.aces.is_empty() && file.sddl.is_none());
        }

        #[test]
        fn a_junction_reports_its_own_descriptor_not_its_target() {
            let dir = tempfile::tempdir().unwrap();
            let target = dir.path().join("target");
            std::fs::create_dir(&target).unwrap();
            // Inheritance cut on the target only (entries copied, so the test keeps access).
            let status = std::process::Command::new("icacls")
                .arg(&target)
                .arg("/inheritance:d")
                .output()
                .unwrap()
                .status;
            assert!(status.success());
            let link = dir.path().join("link");
            mklink_junction(&link, &target);

            assert!(read(&target, What::OwnerAndDacl).unwrap().protected);
            assert!(!read(&link, What::OwnerAndDacl).unwrap().protected);
            std::fs::remove_dir(&link).unwrap();
        }

        #[test]
        fn a_missing_path_is_an_error() {
            let dir = tempfile::tempdir().unwrap();
            assert!(read(&dir.path().join("nope"), What::Owner).is_err());
        }

        #[test]
        fn system_lookups_find_everyone_and_miss_a_made_up_user() {
            let names = SidNames::default();
            assert!(matches!(
                names.lookup("S-1-1-0", None),
                Account::Found { .. }
            ));
            assert_eq!(names.lookup("S-1-5-21-1-2-3-1001", None), Account::NotFound);
        }
    }
}
