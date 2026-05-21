# HDF5 Plugin

Encoder for HDF5 (`.hdf5`) files.

## Requirements

Install the HDF5 C library:

```sh
# Debian/Ubuntu
sudo apt install libhdf5-dev

# Arch
sudo pacman -S hdf5

# macOS
brew install hdf5
```

## Build

```sh
cargo build --release
cp target/release/libhdf5.so .
```
