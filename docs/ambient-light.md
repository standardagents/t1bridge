# Ambient-light sensing

T1 configuration 2 exposes a standard HID sensor collection. On the tested
MacBookPro13,3, the stock `hid_sensor_hub` and `hid_sensor_als` drivers already
bind it and expose an IIO device named `als`. A separate Apple ALS driver is
not required for this observed path. Keep the legacy iBridge driver block in
place: replacing the configuration-2 stack can disrupt the other T1 functions.

## Desktop discovery

The next core package adds `iio-sensor-proxy` as a required dependency. This
change is not yet in the published v0.1.7 package. The distribution proxy owns
its udev discovery rules, service activation and the system D-Bus interface
`net.hadess.SensorProxy`. T1Bridge needs no additional service, socket, fixed
device path or module-loading rule for the observed sensor.

After installing the updated package and completing the normal installation
reboot, use the distribution tool as your desktop user:

```sh
monitor-sensor --light
```

Cover and uncover the ambient-light sensor beside the camera. Confirm that
readings decrease and recover, then stop monitoring with Ctrl-C. Sensor
discovery alone does not establish that readings are fresh or correctly scaled.

The proxy exposes light readings; it does not change display or keyboard
brightness. Automatic brightness requires a desktop consumer. Omarchy policy
and user controls belong in downstream integration and must preserve manual
brightness preferences. Installing T1Bridge must not rewrite those preferences.

## Validation and remaining work

Initial read-only inspection on 2026-09-09, with Arch kernel
`7.1.9-arch1-2` on MacBookPro13,3, found the stock drivers bound to the T1
sensor in USB configuration 2. The IIO attributes reported raw illuminance
112, scale 1, offset 0 and sampling frequency 5 Hz. This is an initial reading,
not calibrated lux accuracy or a changing-light acceptance test.

The sensor proxy was absent at initial inspection. Before claiming out-of-box
support, validate its discovery and changing-light reports, official-package
installation and upgrade, cold boot, concurrent T1 use, and an Omarchy desktop
consumer. Record evidence for the other supported T1 models separately.

System suspend recovery depends on
[#18](https://github.com/standardagents/t1bridge/issues/18), and runtime power
saving depends on [#19](https://github.com/standardagents/t1bridge/issues/19).
Neither is established by a successful sensor read. The parent delivery issue
is [#17](https://github.com/standardagents/t1bridge/issues/17).

## Attended post-reboot check, 2026-09-11

After the owner reported rebooting, `iio-sensor-proxy 3.9-1` was active and
`monitor-sensor --light` discovered the sensor without manual service startup.
Fresh desktop-interface readings were 119 lux before covering, 66–67 lux while
the owner covered the sensor, and 104–105 lux after uncovering. Initial cached
property values were excluded. This confirms response to changing light after
reboot, not calibrated accuracy or system suspend recovery.

The optional Omarchy package now has an automatic-brightness candidate tracked
in [downstream #1](https://github.com/standardagents/t1bridge-omarchy/issues/1).
Its policy and package lifecycle tests pass; live adjustment acceptance remains
pending. Neither this candidate nor the core dependency change is published yet.
