import json
import os
import socket
import sys
from multiprocessing import resource_tracker, shared_memory
from time import time

import numpy as np
import torch
from segment_anything import SamPredictor, sam_model_registry

HOST = "127.0.0.1"
PORT = 50021


def main():
    print("Loading SAM model...")
    checkpoint_path = "sam_vit_b_01ec64.pth"

    device = "cuda" if torch.cuda.is_available() else "cpu"
    print(f"Using {device=}")

    if not os.path.exists(checkpoint_path):
        print(f"ERROR: SAM checkpoint not found at {os.path.abspath(checkpoint_path)}")
        print(
            f"Download '{checkpoint_path}' from: 'https://github.com/facebookresearch/segment-anything?tab=readme-ov-file#model-checkpoints'"
        )
        sys.exit(1)

    sam = sam_model_registry["vit_b"](checkpoint=checkpoint_path).to(device=device)
    predictor = SamPredictor(sam)

    curr_img_w: int = 0
    curr_img_h: int = 0

    try:
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
            s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            s.bind((HOST, PORT))
            s.listen(1)
            print(f"SAM Daemon listening on {HOST}:{PORT}")

            conn, addr = s.accept()
            with conn:
                print(f"Connected by {addr}")
                buffer = ""
                while True:
                    data = conn.recv(4096)  # blocking
                    print(f"{data=}")
                    if not data:
                        break

                    buffer += data.decode("utf-8")
                    while "\n" in buffer:
                        print("Received new message")
                        line, buffer = buffer.split("\n", 1)
                        if not line:
                            continue

                    try:
                        cmd = json.loads(line)
                        action = cmd.get("action")

                        if action == "set_image":
                            curr_img_w = cmd["width"]
                            curr_img_h = cmd["height"]

                            shm = shared_memory.SharedMemory(name=cmd["shm_name"])
                            resource_tracker.unregister(shm._name, "shared_memory")

                            img_array = np.ndarray(
                                (curr_img_h, curr_img_w, 4),
                                dtype=np.uint8,
                                buffer=shm.buf,
                            )

                            start = time()
                            predictor.set_image(img_array[:, :, :3])
                            print(f"Embedding generated in {time() - start:.3f}s")

                            shm.close()
                            conn.sendall(b"OK")

                        elif action == "click":
                            if (
                                not hasattr(predictor, "features")
                                or predictor.features is None
                            ):
                                print("BUSY")
                                conn.sendall(b"BY")
                                continue

                            x, y = cmd["x"], cmd["y"]
                            masks, _, _ = predictor.predict(
                                point_coords=np.array([[x, y]]),
                                point_labels=np.array([1]),
                                multimask_output=False,
                            )

                            m_shm = shared_memory.SharedMemory(name=cmd["shm_name"])
                            resource_tracker.unregister(m_shm._name, "shared_memory")

                            mask_out = np.ndarray(
                                (curr_img_h, curr_img_w),
                                dtype=np.uint8,
                                buffer=m_shm.buf,
                            )
                            np.copyto(mask_out, (masks[0] * 255).astype(np.uint8))

                            m_shm.close()
                            conn.sendall(b"OK")

                    except Exception as e:
                        print(f"Error processing command: {e}")
                        conn.sendall(b"ER")
    finally:
        print("Exiting SAM...")


if __name__ == "__main__":
    main()
