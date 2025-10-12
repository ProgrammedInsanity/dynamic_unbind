use log::{error, warn};
use std::ffi::CString;
use tracing::info;

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error(
        "Kernel module '{kmod}' is currently in use (usage count: {usage_count}) and cannot be unloaded"
    )]
    UnloadKmodInUse { kmod: String, usage_count: i32 },
    #[error("Kernel module does not exist: {0}")]
    KmodUnavailable(String),
    #[error("Failed to unload kernel module: {0}")]
    FailedUnloadingModule(#[from] kmod::Error),
    #[error("Failed to create kernel module context")]
    CouldNotCreateKmodContext,
    #[error("Failed to load kernel module '{kmod}' (return code: {return_code})")]
    FailedToLoadKmod { kmod: String, return_code: i32 },
    #[error("Failed to prepare kernel module '{kmod}' handle (return code: {return_code})")]
    UnableToCreateModuleHandle { kmod: String, return_code: i32 },
}

pub fn kmod_exists(module_name: &str) -> bool {
    kmod::Context::new()
        .expect("Failed to create libkmod context")
        .module_new_from_name(module_name)
        .is_ok()
}

pub fn kmod_loaded(module_name: &str) -> bool {
    kmod::Context::new()
        .expect("Failed to create libkmod context")
        .modules_loaded()
        .expect("Failed to enumerate loaded kernel modules")
        .any(|module| module.name() == module_name)
}

#[tracing::instrument()]
pub fn load_kmod(module_name: &str) -> Result<(), Error> {
    if kmod_loaded(module_name) {
        return Ok(());
    }

    if !kmod_exists(module_name) {
        return Err(Error::KmodUnavailable(module_name.to_string()));
    }
    info!("Loading kernel module '{}'", module_name);

    unsafe {
        let ctx = kmod_sys::kmod_new(std::ptr::null(), std::ptr::null());
        if ctx.is_null() {
            return Err(Error::CouldNotCreateKmodContext);
        }

        let _ = kmod_sys::kmod_load_resources(ctx);

        let c_name = CString::new(module_name).expect("Failed to turn module_name into cstring");

        let mut module: *mut kmod_sys::kmod_module = std::ptr::null_mut();
        let ret_new = kmod_sys::kmod_module_new_from_name(ctx, c_name.as_ptr(), &mut module);
        if ret_new != 0 || module.is_null() {
            error!(
                "Failed to create kernel module handle for '{module_name}': ret={ret_new}, module_ptr={module:p}"
            );
            if !module.is_null() {
                let _ = kmod_sys::kmod_module_unref(module);
            }
            let _ = kmod_sys::kmod_unref(ctx);
            return Err(Error::UnableToCreateModuleHandle {
                kmod: module_name.to_string(),
                return_code: ret_new,
            });
        }

        let ret_probe = kmod_sys::kmod_module_probe_insert_module(
            module,
            0,
            std::ptr::null(),
            None,
            std::ptr::null(),
            None,
        );

        if ret_probe < 0 {
            error!(
                "Failed to insert kernel module '{module_name}' via kmod_module_probe_insert_module: ret={ret_probe}"
            );
            let _ = kmod_sys::kmod_module_unref(module);
            let _ = kmod_sys::kmod_unref(ctx);
            return Err(Error::FailedToLoadKmod {
                kmod: module_name.to_string(),
                return_code: ret_probe,
            });
        }

        let _ = kmod_sys::kmod_module_unref(module);
        let _ = kmod_sys::kmod_unref(ctx);
    }

    Ok(())
}

#[tracing::instrument(skip_all)]
pub fn unload_kmod(module_name: &str) -> Result<(), Error> {
    info!("Unloading kernel module {module_name}");
    let kmod = match kmod::Context::new()
        .expect("Failed to create libkmod context")
        .module_new_from_name(module_name)
    {
        Ok(kmod) => kmod,
        Err(error) => {
            if let kmod::Error::ModuleFromName = error {
                return Err(Error::KmodUnavailable(module_name.to_string()));
            } else {
                panic!(
                    "Other error than ModuleFromName from module_new_from_name libkmod function: {error:?}"
                )
            };
        }
    };

    for holder in kmod.holders() {
        unload_kmod(
            holder
                .name()
                .to_str()
                .expect("Kernel module holder name is not valid UTF-8"),
        )?
    }

    if kmod.refcount() != 0 {
        return Err(Error::UnloadKmodInUse {
            kmod: module_name.to_string(),
            usage_count: kmod.refcount(),
        });
    }
    kmod.remove_module(0)?;

    Ok(())
}
