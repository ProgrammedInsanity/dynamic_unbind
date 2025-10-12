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

    #[allow(unused)]
    #[serde(rename = "@managed")]
    managed: String,

    source: Source,
}

#[derive(Debug, Deserialize, Clone)]
struct Source {
    address: Address,
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
    dynamic_unbind: DynamicUnbind,
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
    #[error("Duplicate configs for one PCI device: '{configs:#?}'")]
    DuplicateConfigsForPCIDevice { configs: Vec<PciDevice> },
    #[error("Failed to create PCI device: {0}")]
    FailedToCreatePCIDevice(#[from] crate::pci_device::Error),
    #[error("User-specified driver '{driver}' for PCI device: \"{pci_addr}\" does not exist")]
    UserSpecifiedDriverDoesNotExist { pci_addr: String, driver: String },
}

pub struct XMLData {
    raw: Domain,
    pci_devices: Vec<pci_device::PCIDevice>,
}

impl XMLData {
    pub fn new(xml: &str) -> Result<Self, Error> {
        let parsed_xml = serde_xml_rs::from_str::<Domain>(xml)?;
        let pci_devices = retrieve_pci_devices_from_parsed_xml(&parsed_xml)?;
        Ok(XMLData {
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
    for pci_device in &xml.metadata.dynamic_unbind.pci_devices {
        let pci_addr = format!(
            "{}:{}:{}.{}",
            pci_device
                .domain
                .strip_prefix("0x")
                .unwrap_or(&pci_device.domain),
            pci_device.bus.strip_prefix("0x").unwrap_or(&pci_device.bus),
            pci_device
                .slot
                .strip_prefix("0x")
                .unwrap_or(&pci_device.slot),
            pci_device
                .function
                .strip_prefix("0x")
                .unwrap_or(&pci_device.function)
        );
        if pci_device.driver_finder_on_shutdown_method
            == DriverFinderOnShutdownMethod::UserSpecified
            && pci_device.driver.is_none()
        {
            return Err(Error::UserSpecifiedButNoDriverSpecified { pci_addr });
        }

        if pci_device.driver_finder_on_shutdown_method
            == DriverFinderOnShutdownMethod::UserSpecified
            && let Some(user_specified_driver) = &pci_device.driver
            && !crate::kernel_modules::kmod_exists(user_specified_driver)
        {
            return Err(Error::UserSpecifiedDriverDoesNotExist {
                pci_addr,
                driver: user_specified_driver.clone(),
            });
        }
    }

    // What a mess, it is so cursed that rust won't format it anymore.
    Ok(xml
        .devices
        .hostdevs
        .as_ref()
        .unwrap_or(&vec![])
        .iter()
        .map(|host_dev| {
            let configs = xml
                .metadata
                .dynamic_unbind
                .pci_devices
                .iter()
                .filter(|pci_device| {
                    let host_dev_addr = &host_dev.source.address;
                    host_dev_addr.domain == pci_device.domain
                        && host_dev_addr.bus == pci_device.bus
                        && host_dev_addr.slot == pci_device.slot
                        && host_dev_addr.function == pci_device.function
                })
                .map(PciDevice::clone)
                .collect::<Vec<PciDevice>>();

            match configs.len() {
                0 => {
                    warn!(
                        "No configuration found for PCI device {}:{}:{}.{} in metadata section",
                        host_dev
                            .source
                            .address
                            .domain
                            .strip_prefix("0x")
                            .expect("Domain missing 0x prefix"),
                        host_dev
                            .source
                            .address
                            .bus
                            .strip_prefix("0x")
                            .expect("Bus missing 0x prefix"),
                        host_dev
                            .source
                            .address
                            .slot
                            .strip_prefix("0x")
                            .expect("Slot missing 0x prefix"),
                        host_dev
                            .source
                            .address
                            .function
                            .strip_prefix("0x")
                            .expect("Function missing 0x prefix")
                    );
                    Ok(None)
                }
                1 => Ok(Some((
                    host_dev,
                    configs
                        .first()
                        .expect("Missing PCI device configuration entry")
                        .clone(),
                ))),
                _ => Err(Error::DuplicateConfigsForPCIDevice { configs }),
            }
        })
        .try_collect::<Vec<Option<(&HostDev, PciDevice)>>>()?
        .iter()
        .flatten()
        .map(|pair| {
            pci_device::PCIDevice::new(
                format!(
                    "{}:{}:{}.{}",
                    pair.1
                        .domain
                        .strip_prefix("0x")
                        .expect("Domain missing 0x prefix in metadata"),
                    pair.1
                        .bus
                        .strip_prefix("0x")
                        .expect("Bus missing 0x prefix in metadata"),
                    pair.1
                        .slot
                        .strip_prefix("0x")
                        .expect("Slot missing 0x prefix in metadata"),
                    pair.1
                        .function
                        .strip_prefix("0x")
                        .expect("Function missing 0x prefix in metadata"),
                ),
                pair.1.driver.clone(),
                pair.1.driver_finder_on_shutdown_method,
            )
        })
        .try_collect()?)
}

pub fn retrieve_uuid_from_xml(xml: &str) -> String {
    serde_xml_rs::from_str::<Domain>(xml)
        .expect("Failed to parse xml config")
        .uuid
}
