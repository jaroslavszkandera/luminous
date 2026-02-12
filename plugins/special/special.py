import json
import mmap
import os
import struct
import sys


def main():
    if len(sys.argv) < 2:
        sys.stderr.write("Usage: plugin.py <image_path>\n")
        sys.exit(1)

    width = 800
    height = 600
    channels = 4  # RGBA
    required_bytes = width * height * channels

    response = {
        "status": "ready",
        "width": width,
        "height": height,
        "required_bytes": required_bytes,
    }
    print(json.dumps(response))
    sys.stdout.flush()

    try:
        shmem_id = sys.stdin.readline().strip()
    except Exception as e:
        sys.stderr.write(f"Error reading stdin: {e}\n")
        sys.exit(1)

    if not shmem_id:
        sys.stderr.write("Received empty shmem ID\n")
        sys.exit(1)

    try:
        shm_path = shmem_id
        if not os.path.exists(shm_path):
            shm_path = os.path.join("/dev/shm", shmem_id.lstrip("/"))

        with open(shm_path, "r+b") as f:
            with mmap.mmap(f.fileno(), required_bytes) as mm:
                red_pixel = struct.pack("BBBB", 255, 0, 0, 255)
                mm.write(red_pixel * (width * height))

    except Exception as e:
        sys.stderr.write(f"Python Error: {e}\n")
        sys.exit(1)


if __name__ == "__main__":
    main()
