use std::fs::{self, File};
use std::io::{BufRead, BufReader};

use std::path::{Path, PathBuf};

use log::error;

const MODULE_EXTENSIONS: &[&str] = &[".ko.zst", ".ko.xz", ".ko"];

/// Normalize module name: convert hyphens to underscores and stop at dots
/// This mirrors the modname_normalize function in kmod's shared/util.c
fn normalize_module_name(modname: &str) -> String {
    let mut result = String::new();

    for c in modname.chars() {
        match c {
            '-' => result.push('_'),
            '.' => break,
            _ => result.push(c),
        }
    }

    result
}

/// Check if a kernel module exists on the system
/// This function searches for modules in both loadable (modules.dep) and builtin (modules.builtin) indexes
///
/// # Arguments
/// * `module_name` - The name of the module to check (e.g., "amdgpu", "nvidia", "snd_hda_intel")
///
/// # Returns
/// * `true` if the module exists, `false` otherwise
pub fn module_exists(module_name: &str) -> bool {
    module_exists_for_kernel(module_name)
}

fn module_exists_for_kernel(module_name: &str) -> bool {
    get_module_path_for_kernel(module_name).is_some()
}

#[tracing::instrument()]
fn kmod_usage_count(driver_name: &str) -> anyhow::Result<i32> {
    let refcnt_path = Path::new("/sys/module").join(driver_name).join("refcnt");

    let refcnt_str = match fs::read_to_string(refcnt_path) {
        Ok(refcnt_str) => refcnt_str,
        Err(error_msg) => {
            error!("Failed to read reference count: {error_msg}");
            return Err(error_msg.into());
        }
    };

    match refcnt_str.trim().parse::<i32>() {
        Ok(refcnt) => Ok(refcnt),
        Err(error_msg) => {
            error!("Failed to parse reference count '{refcnt_str}' : {error_msg}");
            Err(error_msg.into())
        }
    }
}

#[tracing::instrument()]
fn kmod_get_holders(driver_name: &str) -> anyhow::Result<Vec<PathBuf>> {
    let holder_dir = Path::new("/sys/module").join(driver_name).join("holders");

    match fs::read_dir(holder_dir) {
        Ok(dir) => Ok(dir
            .flatten()
            .filter(|entry| entry.file_name() != "." && entry.file_name() != "..")
            .flat_map(|entry| entry.path().canonicalize())
            .collect()),
        Err(err_msg) => {
            error!("Failed to read holders directory: {err_msg}");
            Err(err_msg.into())
        }
    }
}

#[derive(thiserror::Error, Debug)]
pub enum KmodUnloadingError {
    #[error(
        "Kernel module '{kmod}' is currently in use (usage count: {usage_count}) and cannot be unloaded"
    )]
    KmodInUse { kmod: String, usage_count: i32 },
    #[error("Kernel module does not exist: {0}")]
    KmodUnavailable(String),
    #[error("Could not get holders for kernel module: {0}")]
    CouldNotGetHolders(String),
    #[error("Failed to unload kernel module: {0}")]
    FailedUnloadingModule(String),
}

#[tracing::instrument()]
pub fn unload_kmod(kmod_name: &str) -> Result<(), KmodUnloadingError> {
    use nix::kmod::DeleteModuleFlags;
    use nix::kmod::delete_module;
    use std::ffi::CString;
    if !module_exists(kmod_name) {
        return Err(KmodUnloadingError::KmodUnavailable(kmod_name.to_string()));
    }

    let Ok(holders) = kmod_get_holders(kmod_name) else {
        return Err(KmodUnloadingError::CouldNotGetHolders(
            kmod_name.to_string(),
        ));
    };

    holders.iter().for_each(|holder| {
        let _ = unload_kmod(holder.file_name().unwrap().to_str().unwrap()).map_err(|err| {
            error!(
                "Failed to unload holder module '{}': {}",
                holder.file_name().unwrap().to_str().unwrap(),
                err
            )
        });
    });

    if let usage_count = kmod_usage_count(kmod_name).unwrap()
        && usage_count != 0
    {
        return Err(KmodUnloadingError::KmodInUse {
            kmod: kmod_name.to_string(),
            usage_count,
        });
    }

    let c_module_name = CString::new(kmod_name).unwrap();

    if let Err(err_msg) = delete_module(&c_module_name, DeleteModuleFlags::O_NONBLOCK) {
        error!(
            "Failed to unload module '{kmod_name}': {err_msg} (errno: {})",
            nix::errno::Errno::last()
        );
        return Err(KmodUnloadingError::FailedUnloadingModule(
            kmod_name.to_string(),
        ));
    }

    Ok(())
}

/// Get the module path for a given module name
/// This function searches for modules in both loadable (modules.dep) and builtin (modules.builtin) indexes
///
/// # Arguments
/// * `module_name` - The name of the module to find (e.g., "amdgpu", "nvidia", "snd_hda_intel")
///
/// # Returns
/// * `Some(PathBuf)` containing the module path if found, `None` otherwise
pub fn get_module_path(module_name: &str) -> Option<PathBuf> {
    get_module_path_for_kernel(module_name)
}

fn get_module_path_for_kernel(module_name: &str) -> Option<PathBuf> {
    use nix::sys::utsname::uname;
    let kernel_ver = match uname() {
        Ok(uname) => uname.release().to_string_lossy().to_string(),
        Err(_) => return None,
    };

    let modules_dir = PathBuf::from("/lib/modules").join(&kernel_ver);

    // Check loadable modules (modules.dep)
    if let Some(path) = get_module_path_from_dep(&modules_dir, module_name) {
        return Some(path);
    }

    // Check builtin modules (modules.builtin)
    if let Some(path) = get_module_path_from_builtin(&modules_dir, module_name) {
        return Some(path);
    }

    None
}

/// Get module path from modules.dep (loadable modules)
fn get_module_path_from_dep(modules_dir: &Path, module_name: &str) -> Option<PathBuf> {
    let dep_file = modules_dir.join("modules.dep");

    let Ok(file) = File::open(&dep_file) else {
        return None;
    };
    let reader = BufReader::new(file);

    // Normalize the target module name so that it has the same normalization of the of the files.
    let normalized_target = normalize_module_name(module_name);

    for line in reader.lines() {
        if let Ok(line) = line
            && let Some(colon_pos) = line.find(':')
        {
            let module_path = &line[..colon_pos];

            // Extract module name from path (e.g., "kernel/drivers/gpu/drm/amd/amdgpu/amdgpu.ko.zst")
            if let Some(filename) = Path::new(module_path).file_prefix()
                && let Some(name) = filename.to_str()
            {

                let normalized_name = normalize_module_name(name);

                if normalized_name == normalized_target {
                    return Some(modules_dir.join(module_path));
                }
            }
        }
    }

    None
}

/// Get module path from modules.builtin (builtin modules)
fn get_module_path_from_builtin(modules_dir: &Path, module_name: &str) -> Option<PathBuf> {
    let builtin_file = modules_dir.join("modules.builtin");

    let Ok(file) = File::open(&builtin_file) else {
        return None;
    };

    let reader = BufReader::new(file);

    // Normalize the target module name so that it has the same normalization of the of the files.
    let normalized_target = normalize_module_name(module_name);

    // modules.builtin format: "module_path"
    for line in reader.lines() {
        if let Ok(line) = line
            && let Some(filename) = Path::new(&line).file_prefix()
            && let Some(name) = filename.to_str()
        {
            let normalized_name = normalize_module_name(name);

            if normalized_name == normalized_target {
                return Some(modules_dir.join(line));
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_module_exists() {
        // Test some common modules - these tests will depend on the system
        assert!(module_exists("ext4"));
        assert!(module_exists("tcp_cubic"));
        assert!(module_exists("nvidia"));
        assert!(module_exists("snd_hda_intel"));
        assert!(module_exists("virtio-pci"));
        assert!(module_exists("vfio-pci"));
        assert!(!module_exists("Does not exist"));
    }
}
