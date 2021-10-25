# meter-reader
This repository contains the source code for fetching DSMR 4.2 telegrams from a
Dutch residential electricity meter, and publishing them over Ethernet to an
MQTT broker.

It is meant to be built for a Teensy 4.0, with one of its UARTs connected to the
meter, and one of its SPI controllers connected to an ENC28J60 ethernet
controller.

The subproject `dsmr42` contains a `nostd`-compatible DSMR 4.2 parsing library.
While its code is mostly generic, it contains a few assumptions that are
specific to DSMR 4.2 and my own  meter. It can easily be adapted to other meters
and DSMR versions as well.

The Ethernet code depends on
[geluk/enc28j60](https://github.com/geluk/enc28j60), which I have forked from
[japaric/enc28j60](https://github.com/japaric/enc28j60) in order to incorporate
a few more checks and errata fixes into the driver.

## Implementation notes

The default configuration expects the following pin connections:

|Teensy pin|Peripheral|Peripheral pin|
|---|---|---|
|`9`|`ENC28J60`|`Reset`|
|`10`|`ENC28J60`|`Chip select`|
|`11`|`ENC28J60`|`MOSI`|
|`12`|`ENC28J60`|`MISO`|
|`13`|`ENC28J60`|`SCK`|
|`15`|`Meter`|`TX` (uninverted!)|

Note that by default, DSMR 4.2 produces inverted UART signals.
The default configuration of this repository expects a hardware inverter
to be connected between the meter and the Teensy, but it is also possible to
use the Teensy's own inverter. To enable this, set `DSMR_INVERTED` to `true` in
`meter-reader/main.rs`.