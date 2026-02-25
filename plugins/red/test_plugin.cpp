#include <cstddef>
#include <cstdint>
#include <cstring>
#include <fstream>

#ifdef _WIN32
__declspec(dllexport)
#endif

struct ImageBuffer {
  uint8_t *data;
  size_t len;
  uint32_t width;
  uint32_t height;
  uint32_t channels;
};

extern "C" {
void get_plugin_info(char *name, size_t name_max, char *exts, int exts_max) {
  strncpy(name, "Test Plugin", name_max);
  strncpy(exts, "red;test;special", exts_max);
}

ImageBuffer load_image(const char *path) {
  bool ok = true;
  if (ok) {
    uint32_t width = 1, height = 1, channels = 4;
    size_t size = width * height * channels;
    uint8_t *data = new uint8_t[size];

    // Red pixel for example
    data[0] = 255;
    data[1] = 0;
    data[2] = 0;
    data[3] = 255;
    return ImageBuffer{data, size, width, height, channels};
  }
  return ImageBuffer{nullptr, 0, 0, 0, 0};
}

bool save_image(const char *path, ImageBuffer img) {
  std::ofstream file(path, std::ios::binary);
  if (!file.is_open()) {
    return false;
  }

  file.write(reinterpret_cast<const char *>(&img.width), sizeof(img.width));
  file.write(reinterpret_cast<const char *>(&img.height), sizeof(img.height));
  file.write(reinterpret_cast<const char *>(img.data), img.len);

  return file.good();
}

void free_image(ImageBuffer img) {
  if (img.data) {
    delete[] img.data;
  }
}
} // extern "C"
