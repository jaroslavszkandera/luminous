#ifndef LUMINOUS_PLUGINS_RED_FFI_IMAGE
#define LUMINOUS_PLUGINS_RED_FFI_IMAGE

#include <cstddef>
#include <cstdint>

extern "C" {
struct FfiImage {
  uint8_t *data;
  size_t len;
  uint32_t width;
  uint32_t height;
  uint8_t channels;
};

FfiImage load_image(const char *path);
void free_image(FfiImage img);
}

#endif // LUMINOUS_PLUGINS_RED_FFI_IMAGE
