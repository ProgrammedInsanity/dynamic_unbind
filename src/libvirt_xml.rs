use crate::pci_device;

use log::{error, warn};
use serde::Deserialize;
// I am just going to ignore the possibility of attributes or nodes missing and causing an error

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
pub enum DriverFinderOnShutdownMethod {
    #[serde(rename = "kernel")]
    Kernel,
    #[serde(rename = "store")]
    Store,
    #[serde(rename = "user_specified")]
    UserSpecified,
}

#[derive(Debug, Deserialize)]
struct Domain {
    devices: Devices,
    metadata: Metadata,
    #[serde(rename = "uuid")]
    uuid: String,
}

#[derive(Debug, Deserialize)]
struct Devices {
    #[serde(rename = "hostdev")]
    hostdevs: Option<Vec<HostDev>>,
}

#[derive(Debug, Deserialize, Clone)]
struct HostDev {
    #[allow(unused)]
    #[serde(rename = "@mode")]
    mode: String,

    #[allow(unused)]
    #[serde(rename = "@type")]
    device_type: String,

    #[serde(rename = "@managed")]
    managed: String,

    source: Source,
}

#[derive(Debug, Deserialize, Clone)]
struct Source {
    address: Option<Address>,
}

#[derive(Debug, Deserialize, Clone)]
struct Address {
    #[serde(rename = "@domain")]
    domain: String,

    #[serde(rename = "@bus")]
    bus: String,

    #[serde(rename = "@slot")]
    slot: String,

    #[serde(rename = "@function")]
    function: String,
}

#[derive(Debug, Deserialize)]
struct Metadata {
    #[serde(rename = "dyn:dynamic_unbind")]
    dynamic_unbind: Option<DynamicUnbind>,
}

#[derive(Debug, Deserialize)]
struct DynamicUnbind {
    #[serde(rename = "pci_device")]
    pci_devices: Vec<PciDevice>,
}

#[derive(Debug, Deserialize, Clone)]
struct PciDevice {
    #[serde(rename = "@domain")]
    domain: String,

    #[serde(rename = "@bus")]
    bus: String,

    #[serde(rename = "@slot")]
    slot: String,

    #[serde(rename = "@function")]
    function: String,

    #[serde(rename = "@driver_finder_on_shutdown_method")]
    driver_finder_on_shutdown_method: DriverFinderOnShutdownMethod,

    #[serde(rename = "@driver")]
    driver: Option<String>,
}

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("Failed to parse XML: {0}")]
    SerdeFailedToParse(#[from] serde_xml_rs::Error),
    #[error(
        "PCI device {pci_addr} has driver_finder_on_shutdown_method set to 'user-specified' but no driver specified"
    )]
    UserSpecifiedButNoDriverSpecified { pci_addr: String },
    #[error("Duplicate configs for these PCI devices: '{configs:#?}'")]
    DuplicateConfigsForPCIDevicess { configs: Vec<PciDevice> },
    #[error("Failed to create PCI device: {0}")]
    FailedToCreatePCIDevice(#[from] crate::pci_device::Error),
    #[error("User-specified driver '{driver}' for PCI device: \"{pci_addr}\" does not exist")]
    UserSpecifiedDriverDoesNotExist { pci_addr: String, driver: String },
    #[error("A host device ({host_dev:#?}) was found for dynamic device {dynamic_device:#?} but managed is \"yes\". Set managed to \"no\" to let dynamic unbind manage that device")]
    HostDeviceManagedByLibvirt {
        host_dev: Source,
        dynamic_device: PciDevice,
    },
}

pub struct VMConfiguration {
    raw: Domain,
    pci_devices: Vec<pci_device::PCIDevice>,
}

impl VMConfiguration {
    pub fn new(xml: &str) -> Result<Self, Error> {
        let parsed_xml = serde_xml_rs::from_str::<Domain>(xml)?;
        let pci_devices = retrieve_pci_devices_from_parsed_xml(&parsed_xml)?;
        Ok(VMConfiguration {
            raw: parsed_xml,
            pci_devices,
        })
    }

    pub fn get_vm_uuid(&self) -> &str {
        &self.raw.uuid
    }

    pub fn get_pci_devices(&self) -> &Vec<pci_device::PCIDevice> {
        &self.pci_devices
    }
}

#[tracing::instrument(skip_all)]
fn retrieve_pci_devices_from_parsed_xml(xml: &Domain) -> Result<Vec<pci_device::PCIDevice>, Error> {
    let dynamic_devices = if let Some(dynamic_devices) = &xml.metadata.dynamic_unbind {
        &dynamic_devices.pci_devices
    } else {
        &vec![]
    };

    let duplicates = dynamic_devices
        .iter()
        .map(|dyn_device| {
            dynamic_devices
                .iter()
                .filter(|dyn_device_inner| {
                    dyn_device.domain == dyn_device_inner.domain
                        && dyn_device.bus == dyn_device_inner.bus
                        && dyn_device.slot == dyn_device_inner.slot
                        && dyn_device.function == dyn_device_inner.function
                })
                .collect::<Vec<&PciDevice>>()
        })
        .filter(|group| group.len() > 1)
        .collect::<Vec<Vec<&PciDevice>>>();

    if !duplicates.is_empty() {
        return Err(Error::DuplicateConfigsForPCIDevicess {
            configs: duplicates.iter().map(|group| group[0].clone()).collect(),
        });
    }

    let Some(hostdevs) = &xml.devices.hostdevs else {
        return Ok(vec![]);
    };

    Ok(dynamic_devices
        .iter()
        .map(|dyn_device| find_hostdev_for_dyn_device(dyn_device, hostdevs))
        .try_collect::<Vec<Option<pci_device::PCIDevice>>>()?
        .into_iter()
        .flatten()
        .collect())
}

#[tracing::instrument(skip_all)]
fn find_hostdev_for_dyn_device(
    dynamic_device: &PciDevice,
    hostdevs: &[HostDev],
) -> Result<Option<pci_device::PCIDevice>, Error> {
    let configs = hostdevs
        .iter()
        .filter(|host_dev| {
            host_dev.source.address.is_some()
                && host_dev.source.address.as_ref().unwrap().domain == dynamic_device.domain
                && host_dev.source.address.as_ref().unwrap().bus == dynamic_device.bus
                && host_dev.source.address.as_ref().unwrap().slot == dynamic_device.slot
                && host_dev.source.address.as_ref().unwrap().function == dynamic_device.function
        })
        .map(HostDev::clone)
        .collect::<Vec<HostDev>>();

    match configs.len() {
        0 => {
            warn!(
                "No configuration found for PCI device {}:{}:{}.{} in metadata section",
                dynamic_device
                    .domain
                    .strip_prefix("0x")
                    .expect("Domain missing 0x prefix"),
                dynamic_device
                    .bus
                    .strip_prefix("0x")
                    .expect("Bus missing 0x prefix"),
                dynamic_device
                    .slot
                    .strip_prefix("0x")
                    .expect("Slot missing 0x prefix"),
                dynamic_device
                    .function
                    .strip_prefix("0x")
                    .expect("Function missing 0x prefix")
            );
            return Ok(None);
        }
        1 => {}
        _ => unreachable!(),
    };

    let host_dev = configs.first().unwrap();
    if host_dev.managed == "yes" {
        return Err(Error::HostDeviceManagedByLibvirt {
            host_dev: host_dev.source.clone(),
            dynamic_device: dynamic_device.clone(),
        });
    }

    let pci_addr = format!(
        "{}:{}:{}.{}",
        dynamic_device
            .domain
            .strip_prefix("0x")
            .unwrap_or(&host_dev.source.address.as_ref().unwrap().domain),
        dynamic_device
            .bus
            .strip_prefix("0x")
            .unwrap_or(&host_dev.source.address.as_ref().unwrap().bus),
        dynamic_device
            .slot
            .strip_prefix("0x")
            .unwrap_or(&host_dev.source.address.as_ref().unwrap().slot),
        dynamic_device
            .function
            .strip_prefix("0x")
            .unwrap_or(&host_dev.source.address.as_ref().unwrap().function)
    );
    if dynamic_device.driver_finder_on_shutdown_method
        == DriverFinderOnShutdownMethod::UserSpecified
        && dynamic_device.driver.is_none()
    {
        return Err(Error::UserSpecifiedButNoDriverSpecified { pci_addr });
    }

    if dynamic_device.driver_finder_on_shutdown_method
        == DriverFinderOnShutdownMethod::UserSpecified
        && let Some(user_specified_driver) = &dynamic_device.driver
        && !crate::kernel_modules::kmod_exists(user_specified_driver)
    {
        return Err(Error::UserSpecifiedDriverDoesNotExist {
            pci_addr,
            driver: user_specified_driver.clone(),
        });
    }

    Ok(Some(pci_device::PCIDevice::new(
        format!(
            "{}:{}:{}.{}",
            dynamic_device
                .domain
                .strip_prefix("0x")
                .expect("Domain missing 0x prefix in metadata"),
            dynamic_device
                .bus
                .strip_prefix("0x")
                .expect("Bus missing 0x prefix in metadata"),
            dynamic_device
                .slot
                .strip_prefix("0x")
                .expect("Slot missing 0x prefix in metadata"),
            dynamic_device
                .function
                .strip_prefix("0x")
                .expect("Function missing 0x prefix in metadata"),
        ),
        dynamic_device.driver.clone(),
        dynamic_device.driver_finder_on_shutdown_method,
    )?))
}

pub fn retrieve_uuid_from_xml(xml: &str) -> String {
    serde_xml_rs::from_str::<Domain>(xml)
        .expect("Failed to parse xml config")
        .uuid
}
