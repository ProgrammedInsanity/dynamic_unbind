use log::{error, warn};
use serde::Deserialize;
use std::clone;

use crate::pci_device;
// I am just going ignore the possibility of attributes or nodes missing and causing a error

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
    pub devices: Devices,
    pub metadata: Metadata,
}

#[derive(Debug, Deserialize)]
struct Devices {
    #[serde(rename = "hostdev")]
    pub hostdevs: Vec<HostDev>,
}

#[derive(Debug, Deserialize, Clone)]
struct HostDev {
    #[serde(rename = "@mode")]
    pub mode: String,

    #[serde(rename = "@type")]
    pub device_type: String,

    #[serde(rename = "@managed")]
    pub managed: String,

    pub source: Source,
}

#[derive(Debug, Deserialize, Clone)]
struct Source {
    pub address: Address,
}

#[derive(Debug, Deserialize, Clone)]
struct Address {
    #[serde(rename = "@domain")]
    pub domain: String,

    #[serde(rename = "@bus")]
    pub bus: String,

    #[serde(rename = "@slot")]
    pub slot: String,

    #[serde(rename = "@function")]
    pub function: String,
}

#[derive(Debug, Deserialize)]
struct Metadata {
    #[serde(rename = "dyn:dynamic_unbind")]
    pub dynamic_unbind: DynamicUnbind,
}

#[derive(Debug, Deserialize)]
struct DynamicUnbind {
    #[serde(rename = "pci_device")]
    pub pci_devices: Vec<PciDevice>,
}

#[derive(Debug, Deserialize, Clone)]
struct PciDevice {
    #[serde(rename = "@domain")]
    pub domain: String,

    #[serde(rename = "@bus")]
    pub bus: String,

    #[serde(rename = "@slot")]
    pub slot: String,

    #[serde(rename = "@function")]
    pub function: String,

    #[serde(rename = "@driver_finder_on_shutdown_method")]
    pub driver_finder_on_shutdown_method: DriverFinderOnShutdownMethod,

    #[serde(rename = "@driver")]
    pub driver: Option<String>,
}

#[tracing::instrument(skip_all)]
pub fn parse_xml_and_retrieve_pci_devices(xml: &str) -> Vec<pci_device::PCIDevice> {
    let xml = match serde_xml_rs::from_str::<Domain>(xml) {
        Ok(xml) => xml,
        Err(error) => {
            panic!("Failed to parse XML(Please read the documentation): {error}")
        }
    };

    // What a mess
    xml.devices
        .hostdevs
        .iter()
        .filter(|host_dev| host_dev.managed == "no")
        .flat_map(|host_dev| {
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
                    warn!("No configuration found for PCI device {}:{}:{}.{} in metadata section",
                        host_dev.source.address.domain.strip_prefix("0x").unwrap(),
                        host_dev.source.address.bus.strip_prefix("0x").unwrap(),
                        host_dev.source.address.slot.strip_prefix("0x").unwrap(),
                        host_dev.source.address.function.strip_prefix("0x").unwrap()
                    );
                    None
                }
                1 => {
                    Some((host_dev, configs.first().unwrap().clone()))
                }
                _ => {
                    error!("Error while parsing xml: Duplicate configs for one pci device: '{configs:#?}'",);
                    std::process::exit(1);
                }
            }
        })
        .map(|pair| {
            pci_device::PCIDevice::new(
                format!(
                    "{}:{}:{}.{}",
                    pair.1.domain.strip_prefix("0x").unwrap(),
                    pair.1.bus.strip_prefix("0x").unwrap(),
                    pair.1.slot.strip_prefix("0x").unwrap(),
                    pair.1.function.strip_prefix("0x").unwrap()
                ),
                pair.1.driver,
                pair.1.driver_finder_on_shutdown_method,
            )
        })
        .collect()
}
