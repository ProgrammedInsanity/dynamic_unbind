use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use tracing::instrument;

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("I/O error with nvidia_gpu_info file. Error: '{error}' Path: '{nvidia_gpu_info_path}'")]
    IOErrorNvidiaGpuInfoFile {
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
pub fn get_nvidia_device_path(pci_addr: &str) -> Result<PathBuf, Error> {
    use std::fs;
    // https://github.com/NVIDIA/open-gpu-kernel-modules/discussions/336
    let nvidia_gpu_info_path = Path::new("/proc/driver/nvidia/gpus")
        .join(pci_addr)
        .join("information");

    let info_file =
        fs::File::open(&nvidia_gpu_info_path).map_err(|error| Error::IOErrorNvidiaGpuInfoFile {
            nvidia_gpu_info_path: nvidia_gpu_info_path.clone(),
            error,
        })?;

    let Some(line) = BufReader::new(info_file)
        .lines()
        .map_while(|line| line.ok())
        .find(|line| line.starts_with("Device Minor"))
    else {
        return Err(Error::DeviceMinorNotPresent {
            nvidia_gpu_info_path,
        });
    };

    let minor: u32 = line
        .split(":")
        .last()
        .ok_or_else(|| Error::InvalidDeviceMinorFormat { line: line.clone() })?
        .trim()
        .parse::<u32>()
        .map_err(|error| Error::ParseInt {
            line: line.clone(),
            parse_int_error: error,
        })?;
    let nvidia_device_path = Path::new("/dev").join(format!("nvidia{minor}"));

    if nvidia_device_path.exists() {
        Ok(nvidia_device_path)
    } else {
        Err(Error::NvidiaDeviceMissing(nvidia_device_path))
    }
}
