use log::{error, info, warn};
use std::path::{Path, PathBuf};

use crate::{kernel_modules, libvirt_xml, nvidia};
#[derive(Debug, Clone)]
pub struct PCIDevice {
    /// domain:bus:slot.function
    pub pci_addr: String,
    /// example: /sys/bus/pci/drivers/nvidia
    pub driver: Option<PathBuf>,
    /// example: /dev/dri/card0
    pub dri_device_path: Option<PathBuf>,
    /// example: /dev/dri/renderD128
    pub dri_render_path: Option<PathBuf>,
    /// for NVIDIA proprietary driver. example: /dev/nvidia0
    pub nvidia_device_path: Option<PathBuf>,
    /// Driver specified by the user to be loaded on the device after VM shutdown
    pub user_specified_driver: Option<String>,
    /// Used to tell the program which method to use to find the correct driver to load onto the GPU
    pub driver_finder_on_shutdown_method: libvirt_xml::DriverFinderOnShutdownMethod,
}

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("User-specified driver '{driver}' does not exist")]
    UserSpecifiedDriverDoesNotExist { driver: String },
    #[error("Failed to kill processes using PCI device files: {0}")]
    FailedToKillProcesses(#[from] crate::processes::Error),
    #[error("Kernel module operation failed: {0}")]
    KMod(#[from] crate::kernel_modules::Error),

    #[error("Failed to write to driver_override for PCI device {pci_addr}: {io_error}")]
    FailedToWriteToDriverOverride {
        pci_addr: String,
        io_error: std::io::Error,
    },
    #[error("Driver unbind path does not exist: {path}")]
    UnbindPathNotPresent { path: PathBuf },

    #[error("Failed to write to driver unbind for PCI device {pci_addr}: {io_error}")]
    FailedToWriteToDriverUnbind {
        pci_addr: String,
        io_error: std::io::Error,
    },
    #[error("Failed to write to drivers_probe for PCI device {pci_addr}: {io_error}")]
    FailedToWriteToDriverProbe {
        pci_addr: String,
        io_error: std::io::Error,
    },
    #[error("DRI device path not found for PCI device {pci_addr}")]
    DriDevicePathNotFound { pci_addr: String },
    #[error("DRM uevent path not found: {path} for PCI device {pci_addr}")]
    DrmUeventPathNotFound { path: PathBuf, pci_addr: String },
    #[error("Failed to write '{content}' to uevent file {path}: {io_error}")]
    FailedToWriteToUeventFile {
        path: PathBuf,
        content: String,
        io_error: std::io::Error,
    },
}

impl PCIDevice {
    #[tracing::instrument()]
    pub fn new(
        pci_addr: String,
        user_specified_driver: Option<String>,
        driver_finder_on_shutdown_method: libvirt_xml::DriverFinderOnShutdownMethod,
    ) -> Result<PCIDevice, Error> {
        use crate::dri::{get_dri_device_path, get_dri_render_path};
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

        let nvidia_device_path = match nvidia::get_nvidia_device_path(&pci_addr) {
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
            && let Some(ref user_driver) = user_specified_driver
            && !kernel_modules::kmod_exists(user_driver)
        {
            return Err(Error::UserSpecifiedDriverDoesNotExist {
                driver: user_driver.clone(),
            });
        }

        Ok(PCIDevice {
            pci_addr,
            driver,
            dri_device_path,
            dri_render_path,
            nvidia_device_path,
            driver_finder_on_shutdown_method,
            user_specified_driver,
        })
    }

    #[tracing::instrument(skip_all)]
    pub fn unbind_from_driver(&self) -> Result<(), Error> {
        use crate::processes;

        if self.driver.is_none() {
            return Ok(());
        }

        let mut files: Vec<&PathBuf> = vec![];
        if let Some(dri_device_path) = &self.dri_device_path {
            files.push(dri_device_path);
        }
        if let Some(dri_render_path) = &self.dri_render_path {
            files.push(dri_render_path);
        }
        if let Some(nvidia_device_path) = &self.nvidia_device_path {
            files.push(nvidia_device_path);
        }

        info!("Killing processes using these files: {files:#?}");

        processes::kill_processes_using_files(&files)?;

        if let driver_name = self
            .driver
            .as_ref()
            .expect("PCI device is missing an associated driver path")
            .file_name()
            .expect("Driver path is missing a file name component")
            .to_str()
            .expect("Driver name is not valid UTF-8")
            && driver_name == "nvidia"
        {
            kernel_modules::unload_kmod(driver_name)?;
        }

        self.pci_device_bind_with_driver_override("vfio-pci")?;

        Ok(())
    }

    #[tracing::instrument(skip_all)]
    pub fn write_to_driver_override(&self, driver_name: &str) -> Result<(), Error> {
        use std::fs::write;
        let path = Path::new("/sys/bus/pci/devices/")
            .join(&self.pci_addr)
            .join("driver_override");

        write(path, driver_name).map_err(|error| Error::FailedToWriteToDriverOverride {
            pci_addr: self.pci_addr.clone(),
            io_error: error,
        })?;
        Ok(())
    }

    #[tracing::instrument(skip_all)]
    pub fn pci_device_bind_with_driver_override(&self, driver_name: &str) -> Result<(), Error> {
        self.write_to_driver_override(driver_name)?;

        self.pci_device_rebind()?;

        Ok(())
    }

    #[tracing::instrument(skip_all)]
    pub fn pci_device_unbind(&self) -> Result<(), Error> {
        use std::fs::write;
        let driver = get_driver_path(&self.pci_addr);
        if driver.is_none() {
            return Ok(());
        }

        let unbind_path = driver
            .as_ref()
            .expect("PCI device driver path missing when unbinding")
            .join("unbind");

        if !unbind_path.exists() {
            return Err(Error::UnbindPathNotPresent { path: unbind_path });
        }

        write(unbind_path, &self.pci_addr).map_err(|error| Error::FailedToWriteToDriverUnbind {
            pci_addr: self.pci_addr.clone(),
            io_error: error,
        })?;

        Ok(())
    }

    #[tracing::instrument(skip_all)]
    fn pci_device_rebind(&self) -> Result<(), Error> {
        use std::fs::write;
        self.pci_device_unbind()?;
        write("/sys/bus/pci/drivers_probe", &self.pci_addr).map_err(|error| {
            Error::FailedToWriteToDriverProbe {
                pci_addr: self.pci_addr.clone(),
                io_error: error,
            }
        })?;

        Ok(())
    }
}

#[tracing::instrument()]
pub fn get_driver_path(pci_addr: &str) -> Option<PathBuf> {
    let driver_path = Path::new("/sys/bus/pci/devices/")
        .join(pci_addr)
        .join("driver");
    match driver_path.canonicalize() {
        Err(error) => {
            warn!(
                "Error while resolving symlinks of driver_path: '{}': '{:?}'",
                driver_path
                    .to_str()
                    .expect("Driver path contains invalid UTF-8"),
                error
            );
            None
        }
        Ok(path) => Some(path),
    }
}

#[tracing::instrument(skip_all)]
pub fn unbind_gpu_from_kwin(device: &PCIDevice) -> Result<(), Error> {
    use std::fs;

    let dri_device_path =
        device
            .dri_device_path
            .as_ref()
            .ok_or_else(|| Error::DriDevicePathNotFound {
                pci_addr: device.pci_addr.clone(),
            })?;

    let path = Path::new("/sys/bus/pci/devices/")
        .join(&device.pci_addr)
        .join("drm")
        .join(
            dri_device_path
                .file_name()
                .expect("DRI device path missing filename component"),
        )
        .join("uevent");

    if !path.exists() {
        return Err(Error::DrmUeventPathNotFound {
            path: path.clone(),
            pci_addr: device.pci_addr.clone(),
        });
    }

    fs::write(&path, "remove").map_err(|error| Error::FailedToWriteToUeventFile {
        path: path.clone(),
        content: "remove".to_string(),
        io_error: error,
    })?;

    Ok(())
}
