#!/bin/bash
# ^ This is just a stupid trick to get this script to run on both Linux and Windows

cargo objcopy --release -- -O ihex teensy-test.hex

