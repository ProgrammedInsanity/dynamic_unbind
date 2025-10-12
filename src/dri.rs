use std::path::{Path, PathBuf};

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("{path} does not exist")]
    ByPathNotPresent { path: PathBuf },

    #[error("{path} could not resolve symlink with error: {error}")]
    RenderCouldNotResolveSymlink {
        error: std::io::Error,
        path: PathBuf,
    },

    #[error("{path} could not resolve symlink with error: {error}")]
    DeviceFileCouldNotResolveSymlink {
        error: std::io::Error,
        path: PathBuf,
    },
}

#[tracing::instrument()]
#[log_derive::logfn(err = "Warn")]
pub fn get_dri_device_path(pci_addr: &str) -> Result<PathBuf, Error> {
    let dri_path = Path::new("/dev/dri/by-path");
    if !dri_path.exists() {
        return Err(Error::ByPathNotPresent {
            path: dri_path.to_path_buf(),
        });
    }

    dri_path
        .join(format!("pci-{pci_addr}-card"))
        .canonicalize()
        .map_err(|error| Error::DeviceFileCouldNotResolveSymlink {
            error,
            path: dri_path.join(format!("pci-{pci_addr}-card")),
        })
}

#[tracing::instrument()]
#[log_derive::logfn(err = "Warn")]
pub fn get_dri_render_path(pci_addr: &str) -> Result<PathBuf, Error> {
    let dri_path = Path::new("/dev/dri/by-path");
    if !dri_path.exists() {
        return Err(Error::ByPathNotPresent {
            path: dri_path.to_path_buf(),
        });
    }

    dri_path
        .join(format!("pci-{pci_addr}-render"))
        .canonicalize()
        .map_err(|error| Error::RenderCouldNotResolveSymlink {
            error,
            path: dri_path.join(format!("pci-{pci_addr}-render")),
        })
}
