//! The real per-user registry behind [`crate::defterm::Registry`].

use crate::defterm::Registry;
use anyhow::{Result, bail};
use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_READ, REG_SZ, RRF_RT_REG_SZ, RegCloseKey, RegDeleteKeyValueW, RegDeleteKeyW, RegDeleteTreeW, RegGetValueW,
    RegOpenKeyExW, RegQueryInfoKeyW, RegSetKeyValueW,
};

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// `HKEY_CURRENT_USER`.
#[derive(Debug, Default)]
pub struct CurrentUser;

impl CurrentUser {
    fn open(&self, key: &str) -> Option<HKEY> {
        let k = wide(key);
        let mut h: HKEY = std::ptr::null_mut();
        let r = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, k.as_ptr(), 0, KEY_READ, &mut h) };
        (r == ERROR_SUCCESS).then_some(h)
    }
}

impl Registry for CurrentUser {
    fn get(&self, key: &str, name: &str) -> Option<String> {
        let (k, n) = (wide(key), wide(name));
        let mut len = 0u32;
        let r =
            unsafe { RegGetValueW(HKEY_CURRENT_USER, k.as_ptr(), n.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), std::ptr::null_mut(), &mut len) };
        if r != ERROR_SUCCESS || len == 0 {
            return None;
        }
        let mut buf = vec![0u16; (len as usize).div_ceil(2)];
        let r = unsafe {
            RegGetValueW(HKEY_CURRENT_USER, k.as_ptr(), n.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), buf.as_mut_ptr().cast(), &mut len)
        };
        if r != ERROR_SUCCESS {
            return None;
        }
        let end = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..end]))
    }
    fn set(&mut self, key: &str, name: &str, value: &str) -> Result<()> {
        let (k, n, v) = (wide(key), wide(name), wide(value));
        let r = unsafe { RegSetKeyValueW(HKEY_CURRENT_USER, k.as_ptr(), n.as_ptr(), REG_SZ, v.as_ptr().cast(), (v.len() * 2) as u32) };
        if r != ERROR_SUCCESS {
            bail!(r"could not write HKCU\{key} {name}: error {r}");
        }
        Ok(())
    }
    fn delete_value(&mut self, key: &str, name: &str) -> Result<()> {
        let (k, n) = (wide(key), wide(name));
        let r = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, k.as_ptr(), n.as_ptr()) };
        if r != ERROR_SUCCESS && r != ERROR_FILE_NOT_FOUND {
            bail!(r"could not delete HKCU\{key} {name}: error {r}");
        }
        Ok(())
    }
    fn delete_tree(&mut self, key: &str) -> Result<()> {
        let k = wide(key);
        for r in [unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, k.as_ptr()) }, unsafe { RegDeleteKeyW(HKEY_CURRENT_USER, k.as_ptr()) }] {
            if r != ERROR_SUCCESS && r != ERROR_FILE_NOT_FOUND {
                bail!(r"could not delete HKCU\{key}: error {r}");
            }
        }
        Ok(())
    }
    fn key_exists(&self, key: &str) -> bool {
        self.open(key).map(|h| unsafe { RegCloseKey(h) }).is_some()
    }
    fn is_empty_key(&self, key: &str) -> bool {
        let Some(h) = self.open(key) else { return true };
        let (mut subkeys, mut values) = (0u32, 0u32);
        use std::ptr::{null, null_mut};
        let r = unsafe {
            RegQueryInfoKeyW(
                h,
                null_mut(),
                null_mut(),
                null(),
                &mut subkeys,
                null_mut(),
                null_mut(),
                &mut values,
                null_mut(),
                null_mut(),
                null_mut(),
                null_mut(),
            )
        };
        unsafe { RegCloseKey(h) };
        r == ERROR_SUCCESS && subkeys == 0 && values == 0
    }
}
