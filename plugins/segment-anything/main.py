import json
import socket
from multiprocessing import shared_memory

import numpy as np
from segment_anything import SamPredictor, sam_model_registry

HOST = "127.0.0.1"
PORT = 50021


def main():
    print("Loading SAM model...")
    # TODO: Add missing model exeption
    sam = sam_model_registry["vit_b"](checkpoint="sam_vit_b_01ec64.pth")
    predictor = SamPredictor(sam)

    cur_img_w = 0
    curr_img_h = 0

    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        s.bind((HOST, PORT))
        s.listen()
        print(f"SAM Daemon listening on {HOST}:{PORT}")

        conn, addr = s.accept()
        with conn:
            print(f"Connected by {addr}")
            buffer = ""
            while True:
                data = conn.recv(4096)
                if not data:
                    break

                buffer += data.decode("utf-8")
                while "\n" in buffer:
                    print("Received new message")
                    line, buffer = buffer.split("\n", 1)
                    if not line:
                        continue

                    cmd = json.loads(line)
                    print(f"{cmd=}")

                    if cmd["action"] == "set_image":
                        cur_img_w = cmd["width"]
                        curr_img_h = cmd["height"]

                        shm_name = cmd["shm_name"]
                        shm = shared_memory.SharedMemory(name=shm_name)
                        img_array = np.ndarray(
                            (curr_img_h, cur_img_w, 4),
                            dtype=np.uint8,
                            buffer=shm.buf,
                        )
                        predictor.set_image(img_array[:, :, :3])
                        shm.close()

                        conn.sendall(b"OK")

                    elif cmd["action"] == "click":
                        x, y = cmd["x"], cmd["y"]
                        shm_mask_name = cmd["shm_name"]

                        points = np.array([[x, y]])
                        labels = np.array([1])  # 1 = foreground selection

                        masks, _, _ = predictor.predict(
                            point_coords=points,
                            point_labels=labels,
                            multimask_output=False,
                        )

                        mask_shm = shared_memory.SharedMemory(name=shm_mask_name)
                        mask_out = np.ndarray(
                            (curr_img_h, cur_img_w),
                            dtype=np.uint8,
                            buffer=mask_shm.buf,
                        )

                        np.copyto(mask_out, (masks[0] * 255).astype(np.uint8))
                        mask_shm.close()

                        conn.sendall(b"OK")


if __name__ == "__main__":
    main()
