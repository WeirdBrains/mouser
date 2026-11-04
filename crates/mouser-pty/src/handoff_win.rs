//! Mouser's `ITerminalHandoff3` COM local server (docs/windows.md §Default terminal).
//!
//! COM starts `mouser.exe -Embedding` when Windows Terminal's OpenConsole asks
//! for MOUSER_HANDOFF_CLSID. We register a single-use class object, wait for
//! `EstablishPtyHandoff`, create input/output pipes (the console's ends go back
//! as the `[out]` handles; COM closes ours), duplicate the signal, reference,
//! server and client handles (COM closes the lent ones on return) and hand the
//! result over as `Handoff`. `ITerminalHandoff2` (older WT, pipes passed in) too.

use crate::defterm::MOUSER_HANDOFF_CLSID;
use crate::handoff::{Handoff, handles_from_pipes};
use anyhow::{Result, anyhow, bail};
use portable_pty::{Child, ChildKiller, ExitStatus};
use std::ffi::c_void;
use std::fs::File;
use std::os::windows::io::{AsRawHandle, FromRawHandle, IntoRawHandle, OwnedHandle, RawHandle};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, mpsc};
use std::time::Duration;
use windows_sys::Win32::Foundation::{
    CLASS_E_NOAGGREGATION, DUPLICATE_SAME_ACCESS, DuplicateHandle, E_NOINTERFACE, E_POINTER, E_UNEXPECTED, HANDLE, S_OK, SysStringLen, WAIT_OBJECT_0,
};
use windows_sys::Win32::System::Com::{
    CLSCTX_LOCAL_SERVER, COINIT_MULTITHREADED, CoInitializeEx, CoRegisterClassObject, CoRevokeClassObject, CoUninitialize, REGCLS_SINGLEUSE,
};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetExitCodeProcess, GetProcessId, INFINITE, TerminateProcess, WaitForMultipleObjects,
};
use windows_sys::core::{BSTR, GUID, HRESULT};

const IID_IUNKNOWN: GUID = GUID::from_u128(0x00000000_0000_0000_C000_000000000046);
const IID_ICLASSFACTORY: GUID = GUID::from_u128(0x00000001_0000_0000_C000_000000000046);
const IID_HANDOFF2: GUID = GUID::from_u128(0xAA6B364F_4A50_4176_9002_0AE755E7B5EF);
const IID_HANDOFF3: GUID = GUID::from_u128(0x6F23DA90_15C5_4203_9DB0_64E73F1B1B00);
const CLSID_MOUSER: GUID = GUID::from_u128(0xEE272979_892E_4723_9CE6_56F6C13F92EE);
const PIPE_BUFFER: u32 = 128 * 1024;

fn same(a: &GUID, b: &GUID) -> bool {
    a.data1 == b.data1 && a.data2 == b.data2 && a.data3 == b.data3 && a.data4 == b.data4
}

/// `TERMINAL_STARTUP_INFO` from microsoft/terminal `ITerminalHandoff.idl`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct TerminalStartupInfo {
    title: BSTR,
    icon_path: BSTR,
    icon_index: i32,
    x: u32,
    y: u32,
    x_size: u32,
    y_size: u32,
    x_count_chars: u32,
    y_count_chars: u32,
    fill_attribute: u32,
    flags: u32,
    show_window: u16,
}

fn bstr(b: BSTR) -> String {
    if b.is_null() {
        return String::new();
    }
    let n = unsafe { SysStringLen(b) } as usize;
    String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(b, n) })
}

type QueryInterface = unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> HRESULT;
type AddRef = unsafe extern "system" fn(*mut c_void) -> u32;
type Release = unsafe extern "system" fn(*mut c_void) -> u32;

#[repr(C)]
struct FactoryVtbl {
    query_interface: QueryInterface,
    add_ref: AddRef,
    release: Release,
    create_instance: unsafe extern "system" fn(*mut c_void, *mut c_void, *const GUID, *mut *mut c_void) -> HRESULT,
    lock_server: unsafe extern "system" fn(*mut c_void, i32) -> HRESULT,
}

#[repr(C)]
struct Handoff3Vtbl {
    query_interface: QueryInterface,
    add_ref: AddRef,
    release: Release,
    establish:
        unsafe extern "system" fn(*mut c_void, *mut HANDLE, *mut HANDLE, HANDLE, HANDLE, HANDLE, HANDLE, *const TerminalStartupInfo) -> HRESULT,
}

#[repr(C)]
struct Handoff2Vtbl {
    query_interface: QueryInterface,
    add_ref: AddRef,
    release: Release,
    establish: unsafe extern "system" fn(*mut c_void, HANDLE, HANDLE, HANDLE, HANDLE, HANDLE, HANDLE, TerminalStartupInfo) -> HRESULT,
}

#[repr(C)]
struct Factory {
    vtbl: &'static FactoryVtbl,
}

/// One allocation, two interface pointers (v3 first = IUnknown identity).
#[repr(C)]
struct Object {
    v3: &'static Handoff3Vtbl,
    v2: &'static Handoff2Vtbl,
    refs: AtomicU32,
}

static FACTORY_VTBL: FactoryVtbl =
    FactoryVtbl { query_interface: factory_qi, add_ref: static_add_ref, release: static_release, create_instance, lock_server };
static FACTORY: Factory = Factory { vtbl: &FACTORY_VTBL };
static V3: Handoff3Vtbl = Handoff3Vtbl { query_interface: obj_qi, add_ref: obj_add_ref, release: obj_release, establish: establish3 };
static V2: Handoff2Vtbl = Handoff2Vtbl { query_interface: obj_qi2, add_ref: obj_add_ref2, release: obj_release2, establish: establish2 };
static DELIVER: Mutex<Option<mpsc::Sender<Result<Handoff>>>> = Mutex::new(None);

unsafe extern "system" fn static_add_ref(_: *mut c_void) -> u32 {
    2
}
unsafe extern "system" fn static_release(_: *mut c_void) -> u32 {
    1
}
unsafe extern "system" fn factory_qi(this: *mut c_void, iid: *const GUID, out: *mut *mut c_void) -> HRESULT {
    if out.is_null() || iid.is_null() {
        return E_POINTER;
    }
    let iid = unsafe { &*iid };
    unsafe { *out = if same(iid, &IID_IUNKNOWN) || same(iid, &IID_ICLASSFACTORY) { this } else { std::ptr::null_mut() } };
    if unsafe { *out }.is_null() { E_NOINTERFACE } else { S_OK }
}
unsafe extern "system" fn create_instance(_: *mut c_void, outer: *mut c_void, iid: *const GUID, out: *mut *mut c_void) -> HRESULT {
    if out.is_null() {
        return E_POINTER;
    }
    unsafe { *out = std::ptr::null_mut() };
    if !outer.is_null() {
        return CLASS_E_NOAGGREGATION;
    }
    let obj = Box::into_raw(Box::new(Object { v3: &V3, v2: &V2, refs: AtomicU32::new(1) }));
    let hr = unsafe { obj_qi(obj.cast(), iid, out) };
    unsafe { obj_release(obj.cast()) };
    hr
}
unsafe extern "system" fn lock_server(_: *mut c_void, _: i32) -> HRESULT {
    S_OK
}
fn from_v2(this: *mut c_void) -> *mut c_void {
    unsafe { this.cast::<u8>().sub(std::mem::offset_of!(Object, v2)).cast() }
}
unsafe extern "system" fn obj_qi(this: *mut c_void, iid: *const GUID, out: *mut *mut c_void) -> HRESULT {
    if out.is_null() || iid.is_null() {
        return E_POINTER;
    }
    let obj = this.cast::<Object>();
    let iid = unsafe { &*iid };
    let p: *mut c_void = if same(iid, &IID_IUNKNOWN) || same(iid, &IID_HANDOFF3) {
        obj.cast()
    } else if same(iid, &IID_HANDOFF2) {
        unsafe { (&raw mut (*obj).v2).cast() }
    } else {
        unsafe { *out = std::ptr::null_mut() };
        return E_NOINTERFACE;
    };
    unsafe { (*obj).refs.fetch_add(1, Ordering::AcqRel) };
    unsafe { *out = p };
    S_OK
}
unsafe extern "system" fn obj_add_ref(this: *mut c_void) -> u32 {
    unsafe { (*this.cast::<Object>()).refs.fetch_add(1, Ordering::AcqRel) + 1 }
}
unsafe extern "system" fn obj_release(this: *mut c_void) -> u32 {
    let obj = this.cast::<Object>();
    let left = unsafe { (*obj).refs.fetch_sub(1, Ordering::AcqRel) } - 1;
    if left == 0 {
        drop(unsafe { Box::from_raw(obj) });
    }
    left
}
unsafe extern "system" fn obj_qi2(this: *mut c_void, iid: *const GUID, out: *mut *mut c_void) -> HRESULT {
    unsafe { obj_qi(from_v2(this), iid, out) }
}
unsafe extern "system" fn obj_add_ref2(this: *mut c_void) -> u32 {
    unsafe { obj_add_ref(from_v2(this)) }
}
unsafe extern "system" fn obj_release2(this: *mut c_void) -> u32 {
    unsafe { obj_release(from_v2(this)) }
}

fn dup(h: HANDLE) -> Result<OwnedHandle> {
    if h.is_null() {
        bail!("missing handle");
    }
    let mut out: HANDLE = std::ptr::null_mut();
    if unsafe { DuplicateHandle(GetCurrentProcess(), h, GetCurrentProcess(), &mut out, 0, 0, DUPLICATE_SAME_ACCESS) } == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(unsafe { OwnedHandle::from_raw_handle(out as RawHandle) })
}
fn pipe() -> Result<(OwnedHandle, OwnedHandle)> {
    let (mut r, mut w): (HANDLE, HANDLE) = (std::ptr::null_mut(), std::ptr::null_mut());
    if unsafe { CreatePipe(&mut r, &mut w, std::ptr::null(), PIPE_BUFFER) } == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(unsafe { (OwnedHandle::from_raw_handle(r as RawHandle), OwnedHandle::from_raw_handle(w as RawHandle)) })
}
fn raw(h: &OwnedHandle) -> HANDLE {
    h.as_raw_handle() as HANDLE
}
fn terminate(h: &OwnedHandle) -> std::io::Result<()> {
    if unsafe { TerminateProcess(raw(h), 1) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// The console program (client) and OpenConsole (server); ends when either exits.
#[derive(Debug)]
struct HandoffChild {
    client: OwnedHandle,
    server: OwnedHandle,
    _reference: OwnedHandle,
}
#[derive(Debug)]
struct ServerKiller(OwnedHandle);
impl ChildKiller for ServerKiller {
    fn kill(&mut self) -> std::io::Result<()> {
        terminate(&self.0)
    }
    fn clone_killer(&self) -> Box<dyn ChildKiller + Send + Sync> {
        Box::new(ServerKiller(self.0.try_clone().expect("duplicate process handle")))
    }
}
impl HandoffChild {
    fn wait_ms(&self, ms: u32) -> Option<ExitStatus> {
        let hs = [raw(&self.client), raw(&self.server)];
        let r = unsafe { WaitForMultipleObjects(2, hs.as_ptr(), 0, ms) };
        if r != WAIT_OBJECT_0 && r != WAIT_OBJECT_0 + 1 {
            return None;
        }
        let mut code = 0u32;
        unsafe { GetExitCodeProcess(raw(&self.client), &mut code) };
        Some(ExitStatus::with_exit_code(if code == 0x103 { 1 } else { code }))
    }
}
impl ChildKiller for HandoffChild {
    fn kill(&mut self) -> std::io::Result<()> {
        terminate(&self.server)
    }
    fn clone_killer(&self) -> Box<dyn ChildKiller + Send + Sync> {
        Box::new(ServerKiller(self.server.try_clone().expect("duplicate process handle")))
    }
}
impl Child for HandoffChild {
    fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        Ok(self.wait_ms(0))
    }
    fn wait(&mut self) -> std::io::Result<ExitStatus> {
        Ok(self.wait_ms(INFINITE).unwrap_or_else(|| ExitStatus::with_exit_code(1)))
    }
    fn process_id(&self) -> Option<u32> {
        Some(unsafe { GetProcessId(raw(&self.client)) }).filter(|p| *p != 0)
    }
    fn as_raw_handle(&self) -> Option<RawHandle> {
        Some(self.client.as_raw_handle())
    }
}

struct Lent {
    signal: HANDLE,
    reference: HANDLE,
    server: HANDLE,
    client: HANDLE,
}
fn build(output: OwnedHandle, input: OwnedHandle, lent: &Lent, title: String) -> Result<Handoff> {
    let child = HandoffChild { client: dup(lent.client)?, server: dup(lent.server)?, _reference: dup(lent.reference)? };
    let signal = File::from(dup(lent.signal)?);
    let handles = handles_from_pipes(Box::new(File::from(output)), Box::new(File::from(input)), Box::new(signal), Box::new(child))?;
    Ok(Handoff { title, handles })
}
fn deliver(r: Result<Handoff>) -> HRESULT {
    let ok = r.is_ok();
    let sent = DELIVER.lock().unwrap().take().is_some_and(|tx| tx.send(r).is_ok());
    if ok && sent { S_OK } else { E_UNEXPECTED }
}

#[allow(clippy::too_many_arguments)]
unsafe extern "system" fn establish3(
    _: *mut c_void,
    in_: *mut HANDLE,
    out: *mut HANDLE,
    signal: HANDLE,
    reference: HANDLE,
    server: HANDLE,
    client: HANDLE,
    info: *const TerminalStartupInfo,
) -> HRESULT {
    if in_.is_null() || out.is_null() {
        return E_POINTER;
    }
    let title = if info.is_null() { String::new() } else { bstr(unsafe { (*info).title }) };
    let lent = Lent { signal, reference, server, client };
    let r = (|| -> Result<(Handoff, OwnedHandle, OwnedHandle)> {
        let (in_r, in_w) = pipe()?; // console reads input from in_r
        let (out_r, out_w) = pipe()?; // console writes output to out_w
        Ok((build(out_r, in_w, &lent, title)?, in_r, out_w))
    })();
    match r {
        Ok((h, in_r, out_w)) => {
            let hr = deliver(Ok(h));
            if hr == S_OK {
                unsafe {
                    *in_ = in_r.into_raw_handle() as HANDLE;
                    *out = out_w.into_raw_handle() as HANDLE;
                }
            }
            hr
        }
        Err(e) => deliver(Err(e)),
    }
}

#[allow(clippy::too_many_arguments)]
unsafe extern "system" fn establish2(
    _: *mut c_void,
    in_: HANDLE,
    out: HANDLE,
    signal: HANDLE,
    reference: HANDLE,
    server: HANDLE,
    client: HANDLE,
    info: TerminalStartupInfo,
) -> HRESULT {
    let lent = Lent { signal, reference, server, client };
    // v2: the terminal writes input to `in_` and reads output from `out`.
    deliver((|| build(dup(out)?, dup(in_)?, &lent, bstr(info.title)))())
}

/// Serve one handoff: register the class object on an MTA thread (so the UI
/// thread can still OleInitialize), wait up to `timeout`, then revoke.
pub fn receive(timeout: Duration) -> Result<Handoff> {
    let (tx, rx) = mpsc::channel();
    *DELIVER.lock().unwrap() = Some(tx);
    let (reg_tx, reg_rx) = mpsc::channel::<Result<()>>();
    let (done_tx, done_rx) = mpsc::channel::<()>();
    let com = std::thread::spawn(move || {
        unsafe { CoInitializeEx(std::ptr::null(), COINIT_MULTITHREADED as u32) };
        let mut cookie = 0u32;
        let unk = (&raw const FACTORY).cast_mut().cast::<c_void>();
        let hr = unsafe { CoRegisterClassObject(&CLSID_MOUSER, unk, CLSCTX_LOCAL_SERVER, REGCLS_SINGLEUSE as u32, &mut cookie) };
        let _ =
            reg_tx.send(if hr == S_OK { Ok(()) } else { Err(anyhow!("CoRegisterClassObject({MOUSER_HANDOFF_CLSID}) failed: 0x{:08X}", hr as u32)) });
        if hr == S_OK {
            let _ = done_rx.recv();
            unsafe { CoRevokeClassObject(cookie) };
        }
        unsafe { CoUninitialize() };
    });
    let got = reg_rx
        .recv()
        .map_err(|_| anyhow!("COM thread ended"))
        .and_then(|r| r)
        .and_then(|_| rx.recv_timeout(timeout).map_err(|_| anyhow!("no terminal handoff arrived within {timeout:?}"))?);
    let _ = done_tx.send(());
    let _ = com.join();
    got
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    #[test]
    fn clsid_matches_string() {
        let g = CLSID_MOUSER;
        let tail: String = g.data4[2..].iter().map(|b| format!("{b:02X}")).collect();
        let s = format!("{{{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{tail}}}", g.data1, g.data2, g.data3, g.data4[0], g.data4[1]);
        assert_eq!(s, MOUSER_HANDOFF_CLSID);
    }

    fn info() -> TerminalStartupInfo {
        TerminalStartupInfo {
            title: std::ptr::null(),
            icon_path: std::ptr::null(),
            icon_index: 0,
            x: 0,
            y: 0,
            x_size: 0,
            y_size: 0,
            x_count_chars: 0,
            y_count_chars: 0,
            fill_attribute: 0,
            flags: 0,
            show_window: 1,
        }
    }

    /// Drive the object through its vtable as the COM stub would, with real
    /// pipes and this process standing in for both client and OpenConsole.
    #[test]
    fn establish3_hands_back_console_pipes() {
        let (tx, rx) = mpsc::channel();
        *DELIVER.lock().unwrap() = Some(tx);
        let (sig_r, sig_w) = pipe().unwrap();
        let (ref_r, _ref_w) = pipe().unwrap();
        let me = unsafe { GetCurrentProcess() };
        let mut obj: *mut c_void = std::ptr::null_mut();
        let f = (&raw const FACTORY).cast_mut().cast::<c_void>();
        assert_eq!(unsafe { create_instance(f, std::ptr::null_mut(), &IID_HANDOFF3, &mut obj) }, S_OK);
        let (mut cin, mut cout): (HANDLE, HANDLE) = (std::ptr::null_mut(), std::ptr::null_mut());
        let i = info();
        assert_eq!(unsafe { establish3(obj, &mut cin, &mut cout, raw(&sig_w), raw(&ref_r), me, me, &i) }, S_OK);
        unsafe { obj_release(obj) };
        let mut h = rx.recv().unwrap().unwrap();
        let mut console_out = unsafe { File::from_raw_handle(cout as RawHandle) };
        let mut console_in = unsafe { File::from_raw_handle(cin as RawHandle) };
        console_out.write_all(b"C:\\>").unwrap();
        drop(console_out);
        let mut got = Vec::new();
        h.handles.reader.read_to_end(&mut got).unwrap();
        assert_eq!(got, b"C:\\>");
        h.handles.writer.write_all(b"ver\r").unwrap();
        let mut b = [0u8; 4];
        console_in.read_exact(&mut b).unwrap();
        assert_eq!(&b, b"ver\r");
        h.handles.master.resize(portable_pty::PtySize { rows: 30, cols: 100, pixel_width: 0, pixel_height: 0 }).unwrap();
        let mut sig = [0u8; 6];
        File::from(sig_r).read_exact(&mut sig).unwrap();
        assert_eq!(sig, crate::handoff::resize_packet(100, 30));
        assert_eq!(h.handles.child.process_id(), Some(std::process::id()));
    }

    #[test]
    fn interfaces() {
        let mut obj: *mut c_void = std::ptr::null_mut();
        let f = (&raw const FACTORY).cast_mut().cast::<c_void>();
        assert_eq!(unsafe { create_instance(f, std::ptr::null_mut(), &GUID::from_u128(1), &mut obj) }, E_NOINTERFACE);
        assert!(obj.is_null());
        assert_eq!(unsafe { create_instance(f, std::ptr::null_mut(), &IID_HANDOFF2, &mut obj) }, S_OK);
        let mut v3: *mut c_void = std::ptr::null_mut();
        assert_eq!(unsafe { obj_qi2(obj, &IID_HANDOFF3, &mut v3) }, S_OK);
        assert_eq!(v3, from_v2(obj));
        unsafe {
            obj_release(v3);
            obj_release2(obj);
        }
    }
}
