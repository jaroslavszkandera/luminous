# SAM2 Plugin

Interactive image segmentation using [Segment Anything Model 2](https://github.com/facebookresearch/segment-anything). Supports click and selection prompts.

## Requirements

Download a SAM 2 checkpoint (`.pth`) and place it in this directory:

- `sam_vit_b_01ec64.pth` - ViT-B (faster, less accurate)
- `sam_vit_l_0b3195.pth` - ViT-L (slower, more accurate)

Checkpoints are available from the [official repository](https://github.com/facebookresearch/segment-anything#model-checkpoints).

## Setup

```sh
uv sync
```

Dependencies are installed automatically. The daemon starts when Luminous loads the plugin.
