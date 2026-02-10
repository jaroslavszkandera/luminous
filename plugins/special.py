import json
import mmap
import os
import sys


def load_image(path):
    width, height = 500, 500
    pixels = b"\xff\x00\x00\xff" * (width * height)
    return width, height, pixels


def main():
    if len(sys.argv) < 4:
        print(json.dumps({"status": "error", "error": "Missing args"}))
        return

    path = sys.argv[1]
    shmem_id = sys.argv[2]
    shmem_size = int(sys.argv[3])

    try:
        width, height, data = load_image(path)
        data_len = len(data)

        if data_len > shmem_size:
            raise Exception("Image too large for shared memory buffer")

        # Linux
        if os.name == "posix":
            with open(f"/dev/shm/{shmem_id}", "r+b") as f:
                with mmap.mmap(f.fileno(), shmem_size) as shm:
                    shm.seek(0)
                    shm.write(data)
        else:
            # Windows
            with mmap.mmap(-1, shmem_size, tagname=shmem_id) as shm:
                shm.seek(0)
                shm.write(data)

        print(json.dumps({"status": "ok", "width": width, "height": height}))

    except Exception as e:
        print(json.dumps({"status": "error", "error": str(e)}))


if __name__ == "__main__":
    main()
