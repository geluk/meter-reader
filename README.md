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
[https://github.com/geluk/enc28j60](geluk/enc28j60), which I have forked from
[https://github.com/japaric/enc28j60](japaric/enc28j60) in order to incorporate
a few more checks and errata fixes into the driver.
