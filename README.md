# Luminous

Image viewer and editor built with Rust and Slint.

## Quick Start

```bash
cargo run --release -- ./path/to/your/images
```

## Controls

| Key           | Action                         |
| ------------- | ------------------------------ |
| Esc           | Switch between Grid/Full View  |
| q             | Quit Application               |
| f             | Toggle Fullscreen              |
| Left Arrow/h  | Previous Image                 |
| Right Arrow/l | Next Image                     |
| Ctrl + Scroll | Increase/Decrease Grid Columns |
| Scroll        | Navigate Images                |
| PgUp/PgDn     | Scroll Grid Up/Down            |
| Right Click   | Context Menu                   |
| /             | Search                         |
| s             | Toggle Side Panel              |

## Configuration

Luminous supports configuration via command-line arguments or a TOML configuration file.
The application automatically looks for a configuration file at the standard location for your OS:

* Linux/Unix: `~/.config/luminous/luminous.toml`
* Windows: `C:\Users\Username\AppData\Roaming\luminous\luminous.toml`
* macOS: `~/Library/Application Support/luminous/luminous.toml`

Example configuration file with defaults: `examples/luminous.toml`.
