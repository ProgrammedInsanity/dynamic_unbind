#![allow(irrefutable_let_patterns)]
#![feature(path_file_prefix)]
#![feature(iterator_try_collect)]

mod dri;
mod kernel_modules;
mod libvirt_xml;
mod logging;
mod nvidia;
mod pci_device;
mod processes;

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use log::{error, info, warn};

#[tracing::instrument(skip_all)]
fn main() {
    use std::fs;
    use std::path::Path;

    let _ = dotenvy::from_path("/etc/environment");

    fs::create_dir_all(Path::new("/var/log/dynamic_unbind"))
        .expect("Failed to create log directory");

    let libvirt_args = parse_libvirt_args();

    match (
        libvirt_args.operation.as_str(),
        libvirt_args.sub_operation.as_str(),
    ) {
        ("prepare", "begin") => {
            info!("Started prepare begin program");
        }
        ("release", "end") => {
            info!("Started release end program");
        }
        _ => return,
    }

    logging::setup_logging(
        &libvirt_args,
        &libvirt_xml::retrieve_uuid_from_xml(&libvirt_args.xml_config),
    );

    let xml_data =
        libvirt_xml::VMConfiguration::new(&libvirt_args.xml_config).unwrap_or_else(|error| {
            error!("Failed to init XMLData: {error}");
            std::process::exit(1)
        });

    if xml_data.get_pci_devices().is_empty() {
        warn!("No dynamic pci devices were defined")
    }

    match (
        libvirt_args.operation.as_str(),
        libvirt_args.sub_operation.as_str(),
    ) {
        ("prepare", "begin") => begin(&xml_data, &libvirt_args),

        ("release", "end") => end(&xml_data, &libvirt_args),
        _ => {}
    }
}

/// https://www.libvirt.org/hooks.html
#[derive(Clone)]
pub struct LibvirtArgs {
    /// The name of the object involved in the operation, or '-' if there is none. Example: Guest name
    pub object: String,
    /// The name of the operation being performed. Example: prepare
    pub operation: String,
    // Sub-operation indication, or '-' if there is none. Example: begin
    pub sub_operation: String,
    /// extra argument string, or '-' if there is none. Contains shutoff reason when VM is shutdown
    pub extra_argument: String,
    /// XML config of the VM.
    pub xml_config: String,
}

/// https://www.libvirt.org/hooks.html
fn parse_libvirt_args() -> LibvirtArgs {
    #[cfg(not(debug_assertions))]
    {
        use std::io::Read;
        use std::{env, io};
        let mut xml_config = String::new();

        io::stdin()
            .read_to_string(&mut xml_config)
            .expect("Failed to read stdin");
        let mut args: Vec<String> = env::args().skip(1).collect();
        LibvirtArgs {
            object: args.remove(0),
            operation: args.remove(0),
            sub_operation: args.remove(0),
            extra_argument: args.remove(0),
            xml_config,
        }
    }

    #[cfg(debug_assertions)]
    {
        use std::fs;
        LibvirtArgs {
            object: "testVM".to_string(),

            // operation: "prepare".to_string(),
            // sub_operation: "begin".to_string(),
            operation: "release".to_string(),
            sub_operation: "end".to_string(),
            extra_argument: "-".to_string(),
            xml_config: fs::read_to_string("xmlconfig.xml")
                .expect("Failed to read xmlconfig.xml for debug Libvirt arguments"),
        }
    }
}

#[tracing::instrument(skip_all)]
fn begin(xml_data: &libvirt_xml::VMConfiguration, libvirt_args: &LibvirtArgs) {
    if !kernel_modules::kmod_loaded("vfio_pci") {
        info!("Kernel module 'vfio_pci' not loaded; attempting to load it");
        match kernel_modules::load_kmod("vfio_pci") {
            Ok(_) => info!("Successfully loaded kernel module 'vfio_pci'"),
            Err(err) => {
                error!("Failed to load kernel module 'vfio_pci': {err}");
                std::process::exit(1);
            }
        }
    }
    if !kernel_modules::kmod_loaded("vfio_pci") {
        error!("Required vfio_pci kernel module is not loaded.");
        std::process::exit(1);
    }

    let Ok(primary_gpu) = std::env::var("KWIN_DRM_DEVICES") else {
        error!("KWIN_DRM_DEVICES environment variable not found.");
        std::process::exit(1);
    };

    let Some(primary_gpu) = primary_gpu.split(":").next() else {
        error!(
            "Failed to get primary GPU path from KWIN_DRM_DEVICES environment variable. Expected format: '/dev/dri/cardX...' but got: \"{primary_gpu}\""
        );
        std::process::exit(1);
    };

    if xml_data.get_pci_devices().is_empty() {
        warn!("No pci devices specified");
    }

    for device in xml_data.get_pci_devices() {
        info!("Processing PCI device {}", device.pci_addr);
        if let Some(path) = &device.dri_device_path
            && path
                .to_str()
                .expect("DRI device path contains invalid UTF-8")
                == primary_gpu
        {
            error!(
                "Attempting to unbind primary GPU {} which is currently in use by KWin.",
                device.pci_addr
            );
            std::process::exit(1);
        }

        if device.dri_device_path.is_some() {
            let _ = pci_device::unbind_gpu_from_kwin(device).inspect_err(|error| {
                error!(
                    "Failed to unbind GPU {} from KWin with error: \"{error}\"",
                    device.pci_addr
                );
                std::process::exit(1);
            });
        }

        if let Err(e) = device.unbind_from_driver() {
            error!(
                "Failed to unbind PCI device {} from its current driver: {e:?}",
                device.pci_addr
            );
            std::process::exit(1);
        } else {
            info!(
                "Successfully unbound PCI device {} from driver",
                device.pci_addr
            );
        }

        if device.driver_finder_on_shutdown_method
            == libvirt_xml::DriverFinderOnShutdownMethod::Store
        {
            let path = Path::new("/tmp/dynamic_unbind/drivers/");
            let _ = fs::create_dir_all(path);
            let path = path.join(format!(
                "{}_{}_{}",
                libvirt_args.object,
                xml_data.get_vm_uuid(),
                &device.pci_addr
            ));
            let mut file =
                fs::File::create(&path).expect("Failed to create driver cache file for PCI device");

            if let Err(error) = file.write_all(
                serde_json::to_string(&device.driver)
                    .expect("Failed to serialize driver path to JSON")
                    .as_bytes(),
            ) {
                error!(
                    "Failed to write driver information to file for PCI device {} with error: \"{error}\". This will prevent proper driver restoration when the VM shuts down. File path: {}",
                    device.pci_addr,
                    path.display()
                );
                std::process::exit(1);
            } else {
                info!(
                    "Successfully stored driver information for PCI device {} at {}",
                    device.pci_addr,
                    path.display()
                );
            }
        }
    }
}

#[tracing::instrument(skip_all)]
fn end(xml_data: &libvirt_xml::VMConfiguration, libvirt_args: &LibvirtArgs) {
    use libvirt_xml::DriverFinderOnShutdownMethod;

    for device in xml_data.get_pci_devices().iter().filter(|pci_device| {
        matches!(
            pci_device
                .driver
                .as_ref()
                .and_then(|p| p.file_name()),
            Some(name) if name == "vfio-pci"
        )
    }) {
        match &device.driver_finder_on_shutdown_method {
            DriverFinderOnShutdownMethod::Kernel => {
                info!("Trying to load correct driver with kernel method");
                let _ = device
                        .pci_device_bind_with_driver_override("\n")
                        .inspect_err(|error| {
                            error!(
                                "Failed to bind PCI device {} to driver using kernel method with error: \"{error}\". The device may remain unbound and unusable until manual intervention.",
                device.pci_addr
                            );
                        });
            }
            DriverFinderOnShutdownMethod::Store => {
                info!("Trying to load correct driver with store method");
                let path = Path::new("/tmp/dynamic_unbind/drivers/");
                let path = path.join(format!(
                    "{}_{}_{}",
                    libvirt_args.object,
                    xml_data.get_vm_uuid(),
                    &device.pci_addr
                ));

                if !path.exists() {
                    error!(
                        "Stored driver information not found for PCI device {}. Expected file at: '{}'. Cannot restore the original driver without this information.",
                        device.pci_addr,
                        path.display()
                    );
                    std::process::exit(1);
                }

                let driver: Result<Option<PathBuf>, _> = serde_json::from_str(
                    &fs::read_to_string(&path).expect("Failed to read stored driver information"),
                );
                match driver {
                    Err(error) => {
                        error!(
                            "Failed to deserialize stored driver information for PCI device {} from '{}': {error}",
                            device.pci_addr,
                            path.display()
                        );
                        std::process::exit(1);
                    }
                    Ok(driver) => {
                        if let Some(driver) = driver {
                            let driver_name = driver
                                .file_name()
                                .expect("Stored driver path missing filename")
                                .to_str()
                                .expect("Stored driver filename is not valid UTF-8");
                            load_driver_for_device(driver_name, device);
                        } else {
                            // Expected driver path to be present in the stored file but none was found; log a warning and continue.
                            warn!(
                                "Expected stored driver path for PCI device {} but none was found. Skipping driver reload.",
                                device.pci_addr
                            );
                        }
                    }
                }
            }
            DriverFinderOnShutdownMethod::UserSpecified => {
                info!("Trying to load correct driver with user-specified method");
                let driver = device
                    .user_specified_driver
                    .as_ref()
                    .expect("User-specified driver not provided");
                load_driver_for_device(driver, device)
            }
        }
    }
}

#[tracing::instrument]
fn load_driver_for_device(driver_name: &str, device: &pci_device::PCIDevice) {
    // Clear override https://www.kernel.org/doc/Documentation/ABI/testing/sysfs-bus-platform
    info!("Unbinding vfio-pci");
    let _ = device.pci_device_unbind().inspect_err(|error| {
        error!(
            "Error while unbinding vfio-pci from PCI device {}: {error}.",
            device.pci_addr
        );
        std::process::exit(1);
    });
    let _ = device.write_to_driver_override("\n").inspect_err(|error| {
        error!(
            "Could not clear vfio-pci from \"{}\" driver_override: {error}",
            device.pci_addr
        );
        std::process::exit(1);
    });

    if !kernel_modules::kmod_exists(driver_name) {
        error!("Driver does not exist {driver_name}");
        std::process::exit(1);
    }

    if !kernel_modules::kmod_loaded(driver_name) {
        info!(
            "Loading driver '{}' for device {}",
            driver_name, device.pci_addr
        );
        let _ = kernel_modules::load_kmod(driver_name).inspect_err(|error| {
            error!("Failed to load kernel module: {driver_name} with error \"{error}\"");
            std::process::exit(1);
        });
    }

    if driver_name != "nvidia" {
        let _ = device.pci_device_bind_with_driver_override(driver_name).inspect_err(|error| {
            error!("Failed to bind PCI device {} with driver_override method with error \"{error}\"",device.pci_addr);
            std::process::exit(1);
        });
    }

    if driver_name == "nvidia" {
        if let Err(error) = kernel_modules::load_kmod("nvidia_uvm") {
            warn!("Failed to restore NVIDIA CUDA support: {error}");
        }
    }
}
