#![allow(irrefutable_let_patterns)]
#![feature(path_file_prefix)]

mod kernel_modules;
mod libvirt_xml;
mod pci_device;
mod processes;
use std::{
    fs::{self, File},
    path::Path,
};

use log::{error, info};

fn main() {
    use crate::libvirt_xml;
    use std::ffi::CString;
    use std::fs::File;
    use std::io::Read;
    use std::io::prelude::*;

    tracing_subscriber::fmt()
        .with_target(false)
        .with_writer(std::io::stderr)
        .init();
    info!("Started");

    // if !kernel_modules::module_exists("vfio-pci") {
    //     error!("vfio-pci kernel module is not present");
    //     std::process::exit(1);
    // }

    // let vfio_path = kernel_modules::get_module_path("vfio-pci").unwrap();

    // let vfio_file = File::open(&vfio_path).unwrap();

    let file =
        File::open("/usr/lib/modules/6.15.4-1-default/kernel/drivers/vfio/pci/vfio-pci.ko.zst")
            .unwrap();
    let mut decoder = zstd::stream::read::Decoder::new(&file).unwrap();

    let mut module_data = Vec::new();
    decoder.read_to_end(&mut module_data).unwrap();
    let module_params = CString::new("").unwrap();
    let init_result: i32;
    // Explicitly close/drop the file and decoder
    drop(decoder);
    drop(file);

    unsafe {
        std::arch::asm!(
            "syscall",
            in("rax") 175_i64,
            in("rdi") module_data.as_ptr(),
            in("rsi") module_data.len(),
            in("rdx") module_params.as_ptr(),
            lateout("rax") init_result,
        );
    }

    if init_result < 0 {
        let errno = nix::errno::Errno::from_raw(-init_result);
        println!("{init_result:?} {errno}");
    } 

    

    // println!("{init_result:?} {}", nix::errno::Errno::last());

    // let pci_devices = libvirt_xml::parse_xml_and_retrieve_pci_devices(
    //     &fs::read_to_string("xmlconfig.xml").unwrap(),
    // );
    // pci_devices.iter().for_each(|a| {
    //     a.unbind_from_driver();
    // });
}
