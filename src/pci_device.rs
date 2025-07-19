use core::error;
use log::{error, info, warn};
use log_derive::logfn;
use std::{
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
};
use tracing::instrument;

use crate::{kernel_modules, libvirt_xml};
#[derive(Debug, Clone)]
pub struct PCIDevice {
    pci_addr: String,                      // domain:bus:slot.function
    driver: Option<PathBuf>,               // example: /sys/bus/pci/drivers/nvidia
    dri_device_path: Option<PathBuf>,      // example: /dev/dri/card0
    dri_render_path: Option<PathBuf>,      // example: /dev/dri/renderD128
    nvidia_device_path: Option<PathBuf>,   // for NVIDIA proprietary driver. example: /dev/nvidia0
    user_specified_driver: Option<String>, // Driver specified by the user to be loaded on the device after VM shutdown
    driver_finder_on_shutdown_method: libvirt_xml::DriverFinderOnShutdownMethod, // used to tell the program which method to use to find the correct driver to load unto the gpu
}

impl PCIDevice {
    #[tracing::instrument()]
    pub fn new(
        pci_addr: String,
        user_specified_driver: Option<String>,
        driver_finder_on_shutdown_method: libvirt_xml::DriverFinderOnShutdownMethod,
    ) -> PCIDevice {
        let driver = get_driver_path(&pci_addr);

        let dri_device_path = match get_dri_device_path(&pci_addr) {
            Ok(path) => Some(path),
            Err(error) => {
                if let Some(driver) = driver.clone() {
                    let driver_name = driver.file_name().unwrap_or_default().to_string_lossy();
                    if driver_name == "amdgpu" || driver_name == "nvidia" {
                        warn!("Failed to get DRI device path for PCI device {pci_addr}: {error}");
                    }
                }
                None
            }
        };

        let dri_render_path = match get_dri_render_path(&pci_addr) {
            Ok(path) => Some(path),
            Err(error) => {
                if let Some(driver) = driver.clone() {
                    let driver_name = driver.file_name().unwrap_or_default().to_string_lossy();
                    if driver_name == "amdgpu" || driver_name == "nvidia" {
                        warn!("Failed to get DRI render path for PCI device {pci_addr}: {error}");
                    }
                }
                None
            }
        };

        let nvidia_device_path = match get_nvidia_device_path(&pci_addr) {
            Ok(path) => Some(path),
            Err(error) => {
                if let Some(driver) = driver.clone()
                    && driver.ends_with("nvidia")
                {
                    warn!("Failed to get NVIDIA device path for PCI device {pci_addr}: {error}");
                }

                None
            }
        };

        if driver_finder_on_shutdown_method
            == libvirt_xml::DriverFinderOnShutdownMethod::UserSpecified
        {
            if let Some(ref user_driver) = user_specified_driver {
                if !kernel_modules::module_exists(user_driver) {
                    error!(
                        "User specified driver module '{user_driver}' does not exist in the system"
                    );
                }
            } else {
                error!(
                    "Driver finder method is set to UserSpecified but no user_specified_driver was provided"
                );
            }
        }

        PCIDevice {
            pci_addr,
            driver,
            dri_device_path,
            dri_render_path,
            nvidia_device_path,
            driver_finder_on_shutdown_method,
            user_specified_driver,
        }
    }

    #[tracing::instrument(skip_all)]
    pub fn unbind_from_driver(&self) -> Result<(), ()> {
        use crate::processes;

        if self.driver.is_none() {
            error!("");
            return Err(());
        }

        let mut files: Vec<PathBuf> = vec![];
        if let Some(dri_device_path) = &self.dri_device_path {
            files.push(dri_device_path.clone());
        }
        if let Some(dri_render_path) = &self.dri_render_path {
            files.push(dri_render_path.clone());
        }
        if let Some(nvidia_device_path) = &self.nvidia_device_path {
            files.push(nvidia_device_path.clone());
        }

        info!("Killing processes using these files: {files:#?}");

        // processes::kill_processes_using_files(&files);

        let pids: Vec<sysinfo::Pid> = files
            .iter()
            .flat_map(|file| processes::get_pids_via_procfs(file))
            .flatten()
            .collect();

        if pids.is_empty() {
            error!("");
            return Err(());
        }

        if let driver_name = self
            .driver
            .as_ref()
            .unwrap()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            && driver_name == "nvidia"
        {
            let err = kernel_modules::unload_kmod(driver_name);
            error!("");
            return Err(());
        } else {
            pci_device_detatch(&self.pci_addr);
        }


        Ok(())
    }
}

fn pci_device_detatch(pci_addr: &str) {

}

#[derive(thiserror::Error, Debug)]
enum NvidiaPathResolveError {
    #[error("Error with nvidia_gpu_info file. Error: '{error}' Path: '{nvidia_gpu_info_path}'")]
    NvidiaGpuInfoFileError {
        nvidia_gpu_info_path: PathBuf,
        error: std::io::Error,
    },
    #[error("Error while parsing device minor: {parse_int_error} at line: '{line}'")]
    ParseInt {
        line: String,
        parse_int_error: std::num::ParseIntError,
    },
    #[error("Could not find the Device Minor in the file {nvidia_gpu_info_path:#?}")]
    DeviceMinorNotPresent { nvidia_gpu_info_path: PathBuf },
    #[error("nvidia_device file does not exist: {0}")]
    NvidiaDeviceMissing(PathBuf), // The PathBuf is the nvidia_device_path
    #[error("Invalid device minor line format: '{line}'")]
    InvalidDeviceMinorFormat { line: String },
}

#[instrument]
fn get_nvidia_device_path(pci_addr: &str) -> Result<PathBuf, NvidiaPathResolveError> {
    use std::fs;
    // https://github.com/NVIDIA/open-gpu-kernel-modules/discussions/336
    let nvidia_gpu_info_path = Path::new("/proc/driver/nvidia/gpus")
        .join(pci_addr)
        .join("information");

    let info_file = fs::File::open(&nvidia_gpu_info_path).map_err(|error| {
        NvidiaPathResolveError::NvidiaGpuInfoFileError {
            nvidia_gpu_info_path: nvidia_gpu_info_path.clone(),
            error,
        }
    })?;

    let Some(line) = BufReader::new(info_file)
        .lines()
        .map_while(|line| line.ok())
        .find(|line| line.starts_with("Device Minor"))
    else {
        return Err(NvidiaPathResolveError::DeviceMinorNotPresent {
            nvidia_gpu_info_path,
        });
    };

    let minor: u32 = line
        .split(":")
        .last()
        .ok_or_else(|| NvidiaPathResolveError::InvalidDeviceMinorFormat { line: line.clone() })?
        .trim()
        .parse::<u32>()
        .map_err(|error| NvidiaPathResolveError::ParseInt {
            line: line.clone(),
            parse_int_error: error,
        })?;
    let nvidia_device_path = Path::new("/dev").join(format!("nvidia{minor}"));

    if nvidia_device_path.exists() {
        Ok(nvidia_device_path)
    } else {
        Err(NvidiaPathResolveError::NvidiaDeviceMissing(
            nvidia_device_path,
        ))
    }
}

#[instrument]
fn get_driver_path(pci_addr: &str) -> Option<PathBuf> {
    let driver_path = Path::new("/sys/bus/pci/devices/")
        .join(pci_addr)
        .join("driver");
    match driver_path.canonicalize() {
        Err(error) => {
            warn!(
                "Error while resolving driver_path: '{}': '{:?}'",
                driver_path.to_str().unwrap(),
                error
            );
            None
        }
        Ok(path) => Some(path),
    }
}

#[derive(thiserror::Error, Debug)]
enum DriDevicePathResolveError {
    #[error("'/dev/dri/by-path' does not exist")]
    DriByPathNotPresent,
    #[error("dri_device while following symlink")]
    DriDeviceFileError {
        error: std::io::Error,
        path: PathBuf,
    },
}

#[instrument]
#[logfn(err = "Warn")]
fn get_dri_device_path(pci_addr: &str) -> Result<PathBuf, DriDevicePathResolveError> {
    let dri_path = Path::new("/dev/dri/by-path");
    if !dri_path.exists() {
        return Err(DriDevicePathResolveError::DriByPathNotPresent);
    }

    dri_path
        .join(format!("pci-{pci_addr}-card"))
        .canonicalize()
        .map_err(|error| DriDevicePathResolveError::DriDeviceFileError {
            error,
            path: dri_path.join(format!("pci-{pci_addr}-card")),
        })
}

#[derive(thiserror::Error, Debug)]
enum DriRenderPathResolveError {
    #[error("'/dev/dri/by-path' does not exist")]
    DriByPathNotPresent,
    #[error("dri_render while following symlink")]
    DriRenderFileError {
        error: std::io::Error,
        path: PathBuf,
    },
}

#[instrument]
#[logfn(err = "Warn")]
fn get_dri_render_path(pci_addr: &str) -> Result<PathBuf, DriRenderPathResolveError> {
    let dri_path = Path::new("/dev/dri/by-path");
    if !dri_path.exists() {
        return Err(DriRenderPathResolveError::DriByPathNotPresent);
    }

    dri_path
        .join(format!("pci-{pci_addr}-render"))
        .canonicalize()
        .map_err(|error| DriRenderPathResolveError::DriRenderFileError {
            error,
            path: dri_path.join(format!("pci-{pci_addr}-render")),
        })
}
