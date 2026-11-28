//! What Windows says about the computer, from its own APIs: no PowerShell, WMI or `wmic`
//! (which newer Windows 11 releases no longer have), so nothing starts and nothing shows.
#![allow(unsafe_code)] // Win32 calls: each one is given buffers it may write, of their size

use std::os::windows::ffi::OsStrExt as _;
use std::path::Path;

use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, IDXGIFactory1,
};
use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows::core::{PCWSTR, w};

/// Bytes of memory installed (what Windows can use).
pub fn total_memory() -> Option<u64> {
    let mut status = MEMORYSTATUSEX {
        dwLength: u32::try_from(size_of::<MEMORYSTATUSEX>()).ok()?,
        ..Default::default()
    };
    // SAFETY: `status` is a MEMORYSTATUSEX with its size set, as the call requires.
    unsafe { GlobalMemoryStatusEx(&raw mut status) }.ok()?;
    Some(status.ullTotalPhys).filter(|bytes| *bytes > 0)
}

/// The processor's name, like `AMD Ryzen 7 7840U w/ Radeon 780M Graphics`.
pub fn processor_name() -> Option<String> {
    let mut buffer = [0u16; 512];
    let mut bytes = u32::try_from(size_of_val(&buffer)).ok()?;
    // SAFETY: the buffer is `bytes` long and outlives the call.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            w!(r"HARDWARE\DESCRIPTION\System\CentralProcessor\0"),
            w!("ProcessorNameString"),
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr().cast()),
            Some(&raw mut bytes),
        )
    };
    if status.is_err() {
        return None;
    }
    let name = wide_to_string(&buffer);
    // Intel pads its names with spaces.
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    (!name.is_empty()).then_some(name)
}

/// Free bytes for this user on the disk `dir` is on (`dir` must exist).
pub fn free_disk(dir: &Path) -> Option<u64> {
    let wide: Vec<u16> = dir.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut free = 0u64;
    // SAFETY: `wide` is a NUL-terminated path that outlives the call.
    unsafe { GetDiskFreeSpaceExW(PCWSTR(wide.as_ptr()), Some(&raw mut free), None, None) }.ok()?;
    Some(free)
}

/// A graphics adapter, as DirectX lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Adapter {
    pub name: String,
    /// The PCI vendor: `0x10DE` NVIDIA, `0x1002` AMD, `0x8086` Intel.
    pub vendor_id: u32,
    /// Bytes of memory of its own (a few hundred MB, or none, for graphics built into the
    /// processor).
    pub dedicated_memory: u64,
}

/// The hardware graphics adapters (not Microsoft's software renderer), in DirectX's order.
pub fn graphics_adapters() -> Vec<Adapter> {
    // SAFETY: a plain factory call; no COM setup is needed for DXGI.
    let Ok(factory) = (unsafe { CreateDXGIFactory1::<IDXGIFactory1>() }) else {
        return Vec::new();
    };
    let software = u32::try_from(DXGI_ADAPTER_FLAG_SOFTWARE.0).unwrap_or(2);
    let mut adapters = Vec::new();
    for index in 0..16 {
        // SAFETY: enumerating stops at the first index with no adapter.
        let Ok(adapter) = (unsafe { factory.EnumAdapters1(index) }) else {
            break;
        };
        // SAFETY: `adapter` is a live adapter from the factory.
        let Ok(desc) = (unsafe { adapter.GetDesc1() }) else {
            continue;
        };
        if desc.Flags & software != 0 {
            continue;
        }
        adapters.push(Adapter {
            name: wide_to_string(&desc.Description).trim().to_owned(),
            vendor_id: desc.VendorId,
            dedicated_memory: desc.DedicatedVideoMemory as u64,
        });
    }
    adapters
}

fn wide_to_string(wide: &[u16]) -> String {
    let len = wide.iter().position(|&c| c == 0).unwrap_or(wide.len());
    String::from_utf16_lossy(&wide[..len])
}
