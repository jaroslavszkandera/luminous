import inspect
import json
import logging as log
import os
import socket
import sys
from multiprocessing import resource_tracker, shared_memory
from time import time

import numpy as np
import torch
from segment_anything import SamPredictor, sam_model_registry

HOST = "127.0.0.1"
PORT = 50021  # Dynamiclly set port?


def open_shm(name: str) -> shared_memory.SharedMemory:
    log.debug(inspect.stack()[0][3])
    shm = shared_memory.SharedMemory(name=name)
    # Suppress resource tracker warning
    resource_tracker._resource_tracker.unregister(shm._name, "shared_memory")
    return shm


def handle_set_image(cmd: dict, predictor: SamPredictor) -> tuple[int, int]:
    log.debug(inspect.stack()[0][3])
    (w, h) = cmd["width"], cmd["height"]  # TODO: , cmd["channels"]
    shm = open_shm(cmd["shm_name"])
    try:
        img = np.ndarray((h, w, 4), dtype=np.uint8, buffer=shm.buf)
        start = time()
        predictor.set_image(img[:, :, :3])
        log.info(f"Embedding ready in {time() - start:.3f} s ({w}x{h})")
    finally:
        shm.close()
    return w, h


def handle_click(
    cmd: dict,
    predictor: SamPredictor,
    img_w: int,
    img_h: int,
) -> None:
    log.debug(inspect.stack()[0][3])
    x, y = cmd["x"], cmd["y"]
    masks, _, _ = predictor.predict(
        point_coords=np.array([[x, y]]),
        point_labels=np.array([1]),
        multimask_output=False,
    )
    _write_mask(cmd["shm_name"], masks[0], img_w, img_h)


def handle_rect_select(
    cmd: dict,
    predictor: SamPredictor,
    img_w: int,
    img_h: int,
) -> None:
    log.debug(inspect.stack()[0][3])
    x1, y1, x2, y2 = cmd["x1"], cmd["y1"], cmd["x2"], cmd["y2"]
    box = np.array([x1, y1, x2, y2])
    masks, _, _ = predictor.predict(
        box=box[None],
        multimask_output=False,
    )
    _write_mask(cmd["shm_name"], masks[0], img_w, img_h)


def _write_mask(
    shm_name: str,
    mask: np.ndarray,
    img_w: int,
    img_h: int,
) -> None:
    log.debug(inspect.stack()[0][3])
    shm = open_shm(shm_name)
    try:
        out = np.ndarray((img_h, img_w), dtype=np.uint8, buffer=shm.buf)
        np.copyto(out, (mask * 255).astype(np.uint8))
    finally:
        shm.close()


def _embedding_ready(predictor: SamPredictor) -> bool:
    return hasattr(predictor, "features") and predictor.features is not None


def main():
    log.basicConfig(
        format="[SAM]:%(asctime)s:%(levelname)s:%(message)s", level=log.DEBUG
    )
    log.info("Init...")
    device = "cuda" if torch.cuda.is_available() else "cpu"
    log.info(f"Using {device=}")

    log.info("Loading SAM model...")
    checkpoint_path = "sam_vit_b_01ec64.pth"
    if not os.path.exists(checkpoint_path):
        log.critical(
            f"ERROR: SAM checkpoint not found at {os.path.abspath(checkpoint_path)}\n"
            f"Download '{checkpoint_path}' from: 'https://github.com/facebookresearch/segment-anything?tab=readme-ov-file#model-checkpoints'"
        )
        sys.exit(1)

    sam = sam_model_registry["vit_b"](checkpoint=checkpoint_path).to(device=device)
    predictor = SamPredictor(sam)

    curr_img_w: int = 0
    curr_img_h: int = 0

    try:
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as srv:
            srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            srv.bind((HOST, PORT))
            srv.listen(1)
            log.info(f"SAM daemon listening on {HOST}:{PORT}")

            conn, addr = srv.accept()
            conn.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
            log.info(f"Host connected from {addr}")

            with conn:
                buf = ""
                while True:
                    chunk = conn.recv(4096)
                    if not chunk:
                        log.info("Host disconnected.")
                        break
                    buf += chunk.decode("utf-8")

                    # TODO: Switch to length + payload
                    while "\n" in buf:
                        line, buf = buf.split("\n", 1)
                        if not line.strip():
                            continue

                        try:
                            cmd = json.loads(line)
                            action = cmd.get("action")

                            if action == "ping":
                                conn.sendall(b"OK")

                            elif action == "shutdown":
                                conn.sendall(b"OK")
                                return

                            elif action == "set_image":
                                curr_img_w, curr_img_h = handle_set_image(
                                    cmd, predictor
                                )
                                conn.sendall(b"OK")

                            elif action == "click":
                                if not _embedding_ready(predictor):
                                    conn.sendall(b"BY")
                                    continue
                                handle_click(cmd, predictor, curr_img_w, curr_img_h)
                                conn.sendall(b"OK")

                            elif action == "rect_select":
                                if not _embedding_ready(predictor):
                                    conn.sendall(b"BY")
                                    continue
                                handle_rect_select(
                                    cmd, predictor, curr_img_w, curr_img_h
                                )
                                conn.sendall(b"OK")

                            else:
                                log.error(f"Unknown action: {action!r}")
                                conn.sendall(b"ER")

                        except Exception as exc:
                            log.error(f"Error processing command: {exc}")
                            conn.sendall(b"ER")

    finally:
        log.info("SAM daemon exiting...")


if __name__ == "__main__":
    main()
