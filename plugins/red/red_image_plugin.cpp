#include "ffi_image.h"

#include <cstring>
#include <fstream>

// Proof of concept for the load_image rust plugin architecture
FfiImage load_image(const char *path) {
  std::ifstream file(path, std::ios::binary | std::ios::ate);

  if (file.good() && file.tellg() == 0) {
    uint32_t width = 1000;
    uint32_t height = 1000;
    uint8_t channels = 4;
    size_t size = width * height * channels;

    uint8_t *data = new uint8_t[size];
    for (size_t i = 0; i < size; i += 4) {
      data[i] = 255;     // R
      data[i + 1] = 0;   // G
      data[i + 2] = 0;   // B
      data[i + 3] = 255; // A
    }

    return FfiImage{data, size, width, height, channels};
  }
  return FfiImage{nullptr, 0, 0, 0, 0};
}

// Must be called and therefore freed by the host application
void free_image(FfiImage img) {
  if (img.data != nullptr) {
    delete[] img.data;
  }
}
