//! Screen capture through DXGI Desktop Duplication.
//!
//! The GPU hands us only changed frames, so an idle desktop costs next to
//! nothing. Each frame is copied into a CPU-readable staging texture.

use anyhow::{bail, Context, Result};
use windows::core::Interface;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAPPED_SUBRESOURCE,
    D3D11_MAP_READ, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIAdapter1, IDXGIFactory1, IDXGIOutput, IDXGIOutput1,
    IDXGIOutputDuplication, IDXGIResource, DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_NOT_FOUND,
    DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_FRAME_INFO,
};

use super::Display;

struct Output {
    adapter: IDXGIAdapter1,
    output: IDXGIOutput,
    display: Display,
}

fn outputs() -> Result<Vec<Output>> {
    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1()? };
    let mut found = Vec::new();
    for a in 0.. {
        let adapter = match unsafe { factory.EnumAdapters1(a) } {
            Ok(adapter) => adapter,
            Err(e) if e.code() == DXGI_ERROR_NOT_FOUND => break,
            Err(e) => return Err(e.into()),
        };
        for o in 0.. {
            let output = match unsafe { adapter.EnumOutputs(o) } {
                Ok(output) => output,
                Err(e) if e.code() == DXGI_ERROR_NOT_FOUND => break,
                Err(e) => return Err(e.into()),
            };
            let desc = unsafe { output.GetDesc()? };
            if !desc.AttachedToDesktop.as_bool() {
                continue;
            }
            let r = desc.DesktopCoordinates;
            let name_len = desc.DeviceName.iter().position(|&c| c == 0).unwrap_or(32);
            found.push(Output {
                adapter: adapter.clone(),
                output,
                display: Display {
                    index: found.len() as u8,
                    name: String::from_utf16_lossy(&desc.DeviceName[..name_len])
                        .trim_start_matches(r"\\.\")
                        .to_string(),
                    left: r.left,
                    top: r.top,
                    width: (r.right - r.left) as u32,
                    height: (r.bottom - r.top) as u32,
                    primary: r.left == 0 && r.top == 0,
                },
            });
        }
    }
    Ok(found)
}

pub fn displays() -> Result<Vec<Display>> {
    Ok(outputs()?.into_iter().map(|o| o.display).collect())
}

pub struct Capturer {
    display: Display,
    output: IDXGIOutput1,
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    duplication: Option<IDXGIOutputDuplication>,
    /// Since when duplication keeps failing; a fresh device may be needed.
    failing_since: Option<std::time::Instant>,
    staging: Option<ID3D11Texture2D>,
}

// The COM objects are only ever used from the capture thread that owns the Capturer.
unsafe impl Send for Capturer {}

impl Capturer {
    pub fn new(index: u8) -> Result<Self> {
        let Some(out) = outputs()?.into_iter().find(|o| o.display.index == index) else {
            bail!("Bildschirm {index} nicht gefunden");
        };
        let mut device = None;
        let mut context = None;
        unsafe {
            D3D11CreateDevice(
                &out.adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
            .context("Direct3D-Gerät konnte nicht erstellt werden")?;
        }
        let mut capturer = Self {
            display: out.display,
            output: out.output.cast()?,
            device: device.context("kein Direct3D-Gerät")?,
            context: context.context("kein Direct3D-Kontext")?,
            duplication: None,
            failing_since: None,
            staging: None,
        };
        // On a desktop we cannot reach yet (secure desktop), `next_frame` keeps retrying.
        let _ = capturer.duplicate();
        Ok(capturer)
    }

    pub fn display(&self) -> &Display {
        &self.display
    }

    fn duplicate(&mut self) -> Result<()> {
        self.duplication = None;
        // Lock screen and UAC prompts live on another desktop; duplication only sees our thread's.
        crate::desktop::follow_input();
        let dup = unsafe { self.output.DuplicateOutput(&self.device) }
            .context("Bildschirmaufnahme nicht möglich (DuplicateOutput)")?;
        self.duplication = Some(dup);
        Ok(())
    }

    /// Waits up to `timeout_ms` for a changed frame and hands its BGRA pixels
    /// with row pitch and size to `consume`. Returns `false` if nothing changed.
    pub fn next_frame(
        &mut self,
        timeout_ms: u32,
        consume: impl FnOnce(&[u8], usize, u32, u32),
    ) -> Result<bool> {
        if self.duplication.is_none() {
            // Duplication is lost on mode changes and secure-desktop switches; retry quietly.
            if let Err(e) = self.duplicate() {
                let since = *self.failing_since.get_or_insert_with(std::time::Instant::now);
                if since.elapsed() > std::time::Duration::from_secs(2) {
                    // The caller starts over with a new device.
                    return Err(e);
                }
                std::thread::sleep(std::time::Duration::from_millis(timeout_ms.into()));
                return Ok(false);
            }
            self.failing_since = None;
        }
        // A cheap COM reference, so `self` stays free for the copy below.
        let dup = self.duplication.clone().expect("set above");

        let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut resource: Option<IDXGIResource> = None;
        match unsafe { dup.AcquireNextFrame(timeout_ms, &mut info, &mut resource) } {
            Ok(()) => {}
            Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => return Ok(false),
            Err(e) => {
                // ACCESS_LOST on desktop switches (lock screen, UAC, sign-in); other
                // errors there too. Duplicate again on the next call.
                if e.code() != DXGI_ERROR_ACCESS_LOST {
                    tracing::debug!("AcquireNextFrame: {e}");
                }
                self.duplication = None;
                return Ok(false);
            }
        }

        let result = (|| {
            // Mouse-only updates carry no new image.
            if info.LastPresentTime == 0 {
                return Ok(false);
            }
            let texture: ID3D11Texture2D = resource.context("leeres Bild")?.cast()?;
            let staging = self.staging_for(&texture)?;
            unsafe {
                self.context.CopyResource(&staging, &texture);
                let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
                self.context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
                let pitch = mapped.RowPitch as usize;
                let (w, h) = (self.display.width, self.display.height);
                let len = pitch * h as usize;
                consume(std::slice::from_raw_parts(mapped.pData as *const u8, len), pitch, w, h);
                self.context.Unmap(&staging, 0);
            }
            Ok(true)
        })();
        unsafe {
            let _ = dup.ReleaseFrame();
        }
        result
    }

    fn staging_for(&mut self, frame: &ID3D11Texture2D) -> Result<ID3D11Texture2D> {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        unsafe { frame.GetDesc(&mut desc) };
        if let Some(staging) = &self.staging {
            let mut current = D3D11_TEXTURE2D_DESC::default();
            unsafe { staging.GetDesc(&mut current) };
            if current.Width == desc.Width && current.Height == desc.Height {
                return Ok(staging.clone());
            }
        }
        self.display.width = desc.Width;
        self.display.height = desc.Height;
        let staging_desc = D3D11_TEXTURE2D_DESC {
            Width: desc.Width,
            Height: desc.Height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
        };
        let mut staging = None;
        unsafe { self.device.CreateTexture2D(&staging_desc, None, Some(&mut staging))? };
        let staging = staging.context("Staging-Textur fehlt")?;
        self.staging = Some(staging.clone());
        Ok(staging)
    }
}
