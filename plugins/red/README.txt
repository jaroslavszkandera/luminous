# Test plugin that generates a red image when called

## Build
```sh
cd <this-folder>
mkdir build
cmake -S . -B build -DCMAKE_BUILD_TYPE="Release" && cmake --build build && mv build/libred_image_plugin.so .
```
