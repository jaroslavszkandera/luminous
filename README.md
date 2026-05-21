# Luminous

A performant image viewer and editor built with Rust and Slint, with an extensible plugin system.

## Quick Start

Run from source:

```bash
cargo run --release -- ./path/to/your/images
```

For quicker iteration during development:

```bash
cargo run --profile quick-release -- ./path/to/your/images
```

Additional image formats can be enabled with `--features` flag:

```bash
cargo build --release --features <format>
```

Formats decoded via the `image` crate. Enabled by default: `jpeg`, `png`, `webp`, `gif`, `tiff`, `pnm`, `avif`.

## Plugins

Plugins are detected from two locations:
- **Development** (`cargo run`): `<executable>/../../../example_plugins/`
- **Installed** (platform data dir): `~/<data_dir>/luminous/plugins/`
See [ProjectDirs::data_dir](https://docs.rs/directories/latest/directories/struct.ProjectDirs.html#method.data_dir) for platform-specific paths.

### Example Plugins

| Plugin      | Description                         |
| ----------- | ----------------------------------- |
| CLIP        | Semantic image search               |
| HDF5        | Hierarchical Data Format v5 support |
| HEIC        | HEIC format decoding                |
| ARC         | Sony Alpha Raw decoding             |
| SAM2        | Interactive segmentation            |
| SAM3        | Multimodal interactive segmentation |
| WDS         | WebDataset encoding support         |
| test_plugin | Example plugin in C++               |

See each plugin's `README.md` for details.

## Configuration

View all CLI options:

```bash
cargo run --release -- --help
```

The app automatically looks for a TOML config file at:

- Linux:  `~/.config/luminous/luminous.toml`
- Windows: `C:\Users\Username\AppData\Roaming\luminous\luminous.toml`
- macOS: `~/Library/Application Support/luminous/luminous.toml`

An example config with defaults is at `examples/luminous.toml`.

## Controls

Also available in the settings panel (`F1`).

| Key / Mouse                       | Action                  |
| --------------------------------- | ----------------------- |
| Esc / Left Double Click           | Toggle grid / full view |
| Middle / Left Drag                | Pan image               |
| `q`                               | Quit                    |
| `f`                               | Toggle fullscreen       |
| Left Arrow / `h`                  | Previous image          |
| Right Arrow / `l`                 | Next image              |
| Ctrl + Scroll / `-` / `+` (grid)  | Change column count     |
| Ctrl + Scroll / `-` / `+` (image) | Zoom in / out           |
| Scroll                            | Navigate images         |
| PgUp / PgDn                       | Scroll grid up / down   |
| Home / End                        | Grid top / bottom       |
| Right Click                       | Context menu            |
| `z`                               | Reset zoom              |
| `/`                               | Search                  |
| `s`                               | Toggle side panel       |
| `y`                               | Copy to clipboard       |
| Delete                            | Delete                  |
| `F1`                              | Settings / help         |

## License

MIT - see `LICENSE` for details.
