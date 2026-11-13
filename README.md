# dynamic_unbind

This project aims to provide a robust script to unbind the chosen GPU from the DM, driver and loading the vfio pci driver on the GPU. What it does to achieve this is clear logging and error messsages with a lot of checks to **try** and ensure you never get in a state where you need to a reboot. Important this was developed for a system with a AMD gpu and a Nvidia GPU I give no garuantise it will work for other configurations.

The script achieves this by doing lots of checks and extensive logging to ensure that each issue is easily diagnosable. I have been using this for over year and it's so nice not having to muck about in the the file system adding new scripts moving them or fix/program them and instead just configure it in the VM config itself.

## How it works

It makes use of the fact that libvirt passes the whole xml configuration to hook scripts. With this you can write a hook script that works for every vm so that you only have to change the xml config instead of moving files or editing scripts. We cannot add custom xml so the config for dynamic unbind is defined in the metadata section.

```xml
  <metadata>
    ...
    <dyn:dynamic_unbind xmlns:dyn="0.0.0.0">
      <pci_device domain="0x0000" bus="0x0a" slot="0x00" function="0x0" driver_finder_on_shutdown_method="store"/>
    </dyn:dynamic_unbind>
    ...
  </metadata>
```

On the `prepare begin` libvirt event it reads the passed xml acts accordingally. It checks if the GPU is not the primary kwin GPU, it then unbinds the GPU from kwin and kills all the processes using the GPU. After which it unbinds the GPU from the AMD or unloads the Nvidia driver and finally loads the `vfio-pci` driver.

On the `release end` libvirt event it looks up the correct driver for the GPU with the `driver_finder_on_shutdown_method` which can be either `store`, `kernel` or `user_specified`. I then unbinds the GPU from `vfio-pci` and then loads the driver.

The processes are oblitirated with a SIGTERM followed by a SIGKILL after a delay. If it doesn't succeeds in killing all the processes using it will not attempt to unbind the driver and instead aborts the vm start.

## Requirements

- Kwin
- Two GPUs
- A already working PCI passthrough setup

## Setup

### Build

The program is a statically compiled binary. The nix flake provides all the dependencies it needs to compile. If you don't have nix installed then install it and enable flakes.
Then just run this command to build it.

```sh
nix develop -c cargo build --release --target x86_64-unknown-linux-musl
```

Then run this command.

```sh
sudo install -Dm755 target/x86_64-unknown-linux-musl/release/dynamic_unbind /etc/libvirt/hooks/qemu.d/dynamic_unbind
```

This will copy the binary to the destination and create the parents if they don't already exists. For why it needs to go to that directory you can read more about that here: <https://www.libvirt.org/hooks.html>

### Host environment

The hook reads `/etc/environment` using dotenv syntax. Ensure `KWIN_DRM_DEVICES` is available to both the KWin session and the hook. For example, if `card0` is the host GPU and `card1` is the passthrough GPU:

```dotenv
KWIN_DRM_DEVICES=/dev/dri/card0:/dev/dri/card1
```

### VM configuration

Add a `dyn:dynamic_unbind` block inside the domain's `<metadata>` element. Each `pci_device` must have a matching PCI `<hostdev>` source address under `<devices>`, with `managed="no"` so this hook controls driver binding.

The following fragments belong inside the VM's existing `<domain>` element. Replace the example address with your device's PCI address:

```xml
<metadata>
  ...
  <dyn:dynamic_unbind xmlns:dyn="0.0.0.0">
    <pci_device domain="0x0000" bus="0x0a" slot="0x00" function="0x0"
                driver_finder_on_shutdown_method="store"/>
  </dyn:dynamic_unbind>
  ...
</metadata>

<devices>
  ...
  <!-- The GPU you want to pass through it is the same as the pci_device defined in the dynamic_unbind in the metedata node -->
  <hostdev mode="subsystem" type="pci" managed="no">
    <source>
      <address domain="0x0000" bus="0x0a" slot="0x00" function="0x0"/>
    </source>
  </hostdev>
  ...
</devices>
```

Merge these entries into existing `<metadata>` and `<devices>` elements. Use identical address strings in both entries. Duplicate metadata entries and matching host devices with `managed="yes"` cause errors. There is example at `xmlconfig.xml`.

### Driver restoration

| Method | Behavior |
| --- | --- |
| `store` | Records the original driver under `/tmp/dynamic_unbind/drivers/` during preparation and uses it during release. |
| `user_specified` | Uses the kernel module named by the required `driver` attribute, for example `driver="nvidia"`. |
| `kernel` | Clears `driver_override` and reprobes the device, leaving driver selection to the kernel. |

## Logs and troubleshooting

Logs are sent to standard error, the system journal, and files under:

```text
/var/log/dynamic_unbind/<guest-name>-<uuid>/<operation>-<sub-operation>/<UTC-timestamp>.log
```

Each invocation logs the executable's SHA-256 hash to identify the build in use.
