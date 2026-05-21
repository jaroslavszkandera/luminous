# SAM3 Plugin

Interactive image segmentation using [Segment Anything Model 3](https://huggingface.co/facebook/sam3). Supports selection and text prompts. Model weights are downloaded from HuggingFace automatically.

## Requirements

A [HuggingFace](https://huggingface.co/facebook/sam3) account token with access to the SAM3 model. Provide it via one of:

- Environment variable: `HF_TOKEN=<token>`
- File: `~/.hf_token`
- Interactive prompt on first run (GUI or terminal)

## Setup

```sh
uv sync
```

The model is downloaded on first use. The daemon starts when Luminous loads the plugin.
