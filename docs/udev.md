# udev rules (Linux)

The cable's HID endpoint (`/dev/hidraw*`) is root-only by default. Without a
rule, both `auri` and Chromium/WebHID fail to open it as a normal user.

The rules are about **file permissions**, not about linking: `auri` talks to
`/dev/hidraw*` directly, so a static musl build needs them exactly as much as
the glibc one does.

## Install

```sh
sudo tee /etc/udev/rules.d/99-kuba-auri.rules >/dev/null <<'EOF'
# Generic USB permission (hidraw backends)
SUBSYSTEMS=="usb", ATTRS{idVendor}=="3302", ATTRS{idProduct}=="8902", MODE="0666", TAG+="uaccess"
# HID raw node — where the write actually lands
KERNEL=="hidraw*", ATTRS{idVendor}=="3302", ATTRS{idProduct}=="8902", MODE="0666", TAG+="uaccess"
EOF
sudo udevadm control --reload-rules
sudo udevadm trigger
```

Replug the cable, then verify:

```sh
ls -l /dev/hidraw*
# crw-rw-rw- 1 root root ... /dev/hidrawN     (or a `+` marking the uaccess ACL)
cargo run -p auri-cli -- devices
```

`auri devices` prints the device label when the node is openable. `no cable
found (3302:8902)` means the cable is unplugged **or** the rules/permissions
are not in effect.

`MODE="0666"` makes the node world-writable; `TAG+="uaccess"` additionally
grants the active local seat. On a shared machine, drop `MODE="0666"` and rely
on the `uaccess` ACL alone.

## Chromium / WebHID

Chrome lists the device in the picker but `open()` fails with
`NotAllowedError` / "Failed to open HID device" when the udev permissions are
missing. The same rule fixes both the web app and `auri`.

## Two interfaces, no conflict

The cable is a composite USB device. Interface `mi_00` carries audio (UAC) and
is held by PipeWire/ALSA; the equalizer lives on `mi_03` as HID. Opening the
HID node never touches the audio interface, and audio playback does not block a
write. `lsusb -t` shows both interfaces of device `3302:8902`.
