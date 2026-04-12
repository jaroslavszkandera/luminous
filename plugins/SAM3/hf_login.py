"""
HuggingFace authentication helper.

Priority order:
  1. HF_TOKEN environment variable
  2. ~/.hf_token file
  3. Tkinter popup
  4. Terminal prompt
"""

import os
import sys

# TODO: Logging


def login(path: str = "~/.hf_token") -> str:
    token = _from_env() or _from_file(path) or _from_popup() or _from_terminal()
    if not token:
        print("[hf_login] ERROR: No HuggingFace token provided.", file=sys.stderr)
        sys.exit(1)

    from huggingface_hub import login

    login(token=token)
    print("[hf_login] Logged in to HuggingFace.")
    return token


def _from_env() -> str | None:
    token = os.environ.get("HF_TOKEN")
    if token:
        print("[hf_login] Using token from HF_TOKEN env var.")
    return token


def _from_file(path: str = "~/.hf_token") -> str | None:
    expanded = os.path.expanduser(path)
    if os.path.exists(expanded):
        with open(expanded) as f:
            token = f.read().strip()
        if token:
            print(f"[hf_login] Using token from {expanded}")
            return token
    return None


def _from_popup() -> str | None:
    try:
        import tkinter as tk
        from tkinter import simpledialog

        root = tk.Tk()
        root.withdraw()
        root.lift()
        token = simpledialog.askstring(
            "HuggingFace Login",
            "Enter your HuggingFace token:",
            show="*",
            parent=root,
        )
        root.destroy()
        return token.strip() if token else None
    except Exception:
        return None


def _from_terminal() -> str | None:
    try:
        import getpass

        token = getpass.getpass("[hf_login] Enter HuggingFace token: ").strip()
        return token or None
    except Exception:
        return None


if __name__ == "__main__":
    login()
    print("Auth successful.")
