//! Marks the image as an EFI boot-service driver so firmware keeps it
//! resident after its entry point returns (applications are unloaded).

/// Emits the linker flag that sets the PE subsystem.
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("uefi") {
        println!("cargo:rustc-link-arg=/subsystem:efi_boot_service_driver");
    }
}
