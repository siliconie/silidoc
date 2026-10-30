//! Graphics backend choice, applied before wgpu initialises any driver.
//!
//! Windows: DirectX 12 with the FXC shader compiler, as the three engines' own apps do. eframe's
//! default also initialises OpenGL, which can crash inside some AMD drivers before any window
//! appears, and the automatic DXC lookup can pick an unrelated `dxcompiler.dll` (DeckCraft #153).
//! `WGPU_BACKEND` and `WGPU_DX12_COMPILER` still win when set.

#[cfg(target_os = "windows")]
pub fn configure(options: &mut eframe::NativeOptions) {
    use eframe::egui_wgpu::WgpuSetup;
    use eframe::wgpu::{Backends, Dx12Compiler};

    let backend_override = Backends::from_env();
    let compiler_override = Dx12Compiler::from_env();
    if let WgpuSetup::CreateNew(create) = &mut options.wgpu_options.wgpu_setup {
        create.instance_descriptor.backends = backend_override.unwrap_or(Backends::DX12);
        create.instance_descriptor.backend_options.dx12.shader_compiler = compiler_override.unwrap_or(Dx12Compiler::Fxc);
        log::info!(
            "Windows graphics: backends={:?}, DX12 compiler={:?}",
            create.instance_descriptor.backends,
            create.instance_descriptor.backend_options.dx12.shader_compiler
        );
    }
}

#[cfg(not(target_os = "windows"))]
pub fn configure(_options: &mut eframe::NativeOptions) {}
