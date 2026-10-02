# Spacetop

A Wayland compositor that presents app windows as OpenXR panels on Linux.

```sh
cargo run --release -- --app=totem
```

Aim with the right controller and use its trigger to click.

## GPU Rendering

GPU sharing is mandatory and uses the OpenXR runtime's Vulkan GPU.
Smithay composites app buffers using GLES into linear RGBA DMA-BUF images.
Vulkan imports those images and copies them into the OpenXR swapchain without
reading panel pixels back to the CPU. The targeting cross uses a tiny persistent
GPU buffer instead of modifying panel pixels.

GPU-backed Wayland client buffers are supported through `linux-dmabuf` with
device feedback. Shared-memory client buffers still require an upload to GLES.
The GLES renderer and Vulkan session use the same DRM render node; the XR
runtime itself does not need to support GLES.

This initial implementation requires Vulkan 1.1 and the external-memory FD,
DMA-BUF, DRM-format-modifier, physical-device-DRM, image-format-list, and
foreign-queue-family device extensions. It also needs a driver that supports
linear RGBA8 GLES render targets and sRGB Vulkan DMA-BUF imports, an sRGB OpenXR
swapchain, and access to the runtime GPU's `/dev/dri/renderD*` node.
Build dependencies include EGL, GLES, DRM, GBM (with modifier-aware allocation
and per-plane FD export), and xkbcommon development libraries.

Startup logs report the selected render node. Missing GPU capabilities,
renderer initialization failures, panel capture failures, and Vulkan DMA-BUF
import failures stop the compositor with an error. There is no CPU renderer,
panel readback/upload fallback, or renderer-selection environment variable.
App redraws wait for GPU setup to complete.

This is not yet a fully pipelined renderer: each app update allocates a fresh
shared image, producer completion is waited on by the CPU, and Vulkan waits for
copies before releasing swapchain images. Panels remain limited to 512 pixels
per dimension, and only one window is displayed. Buffer pooling, explicit GPU
semaphore handoff, higher resolution, and multi-panel swapchains are follow-ups.

## Tests

```sh
cargo test
cargo clippy --all-targets -- -D warnings
```

An opt-in hardware test creates a real Wayland app, composites through GLES,
imports into Vulkan, checks image-copy and cursor pixels, exercises GPU-backed
client buffers, and verifies deferred GPU setup, focus, clicks, and redraws.
Test-only readback verifies pixels; the running compositor never reads panel
images back to the CPU:

```sh
cargo test gpu_shared_app -- --ignored --nocapture
```

To select a particular GPU for that test:

```sh
SPACETOP_GPU_TEST_NODE=/dev/dri/renderD129 cargo test gpu_shared_app -- --ignored --nocapture
```